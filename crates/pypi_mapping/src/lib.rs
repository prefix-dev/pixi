use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};

use futures::{stream::FuturesUnordered, StreamExt};
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_config::get_cache_dir;
use rattler_conda_types::{PackageUrl, RepoDataRecord};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use thiserror::Error;
use tokio::sync::Semaphore;
use url::Url;

mod custom_mapping;
pub mod prefix;
mod reporter;

pub use custom_mapping::CustomMapping;
pub use reporter::Reporter;

use crate::custom_mapping::CustomMappingClient;

/// A compressed mapping is a mapping of a package name to a potential pypi
/// name.
pub type CompressedMapping = HashMap<String, Option<String>>;

pub type ChannelName = String;

pub type MappingMap = HashMap<ChannelName, MappingLocation>;
pub type MappingByChannel = HashMap<ChannelName, CompressedMapping>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingLocation {
    Path(PathBuf),
    Url(Url),
    Memory(CompressedMapping),
}

/// This enum represents the source of mapping
/// it can be user-defined ( custom )
/// or from prefix.dev ( prefix )
#[derive(Debug, Clone)]
pub enum MappingSource {
    Custom(Arc<CustomMapping>),
    Prefix,
    Disabled,
}

impl MappingSource {
    /// Return the custom `MappingMap`
    /// for `MappingSource::Custom`
    pub fn custom(&self) -> Option<Arc<CustomMapping>> {
        match self {
            MappingSource::Custom(mapping) => Some(mapping.clone()),
            _ => None,
        }
    }
}

/// This enum represents the source of mapping
/// it can be user-defined ( custom )
/// or from prefix.dev ( prefix )
#[derive(Debug, Clone)]
pub enum PurlSource {
    HashMapping,
    CompressedMapping,
    ProjectDefinedMapping,
}

impl PurlSource {
    pub fn as_str(&self) -> &str {
        match self {
            PurlSource::HashMapping => "hash-mapping",
            PurlSource::CompressedMapping => "compressed-mapping",
            PurlSource::ProjectDefinedMapping => "project-defined-mapping",
        }
    }
}

/// Returns `true` if the specified record refers to a conda-forge package.
pub fn is_conda_forge_record(record: &RepoDataRecord) -> bool {
    record
        .channel
        .as_ref()
        .and_then(|channel| Url::from_str(channel).ok())
        .is_some_and(|u| is_conda_forge_url(&u))
}

/// Returns `true` if the specified url refers to a conda-forge channel.
pub fn is_conda_forge_url(url: &Url) -> bool {
    url.path().starts_with("/conda-forge")
}

/// The mapping client implements the logic to derive purls for conda packages.
/// Internally it uses a combination of sources and also allows overwriting the
/// sources for particular channels.
///
/// For more information see:
/// - [`prefix::CompressedMappingClient`]
/// - [`prefix::HashMappingClient`]
/// - [`CondaForgeVerbatim`]
#[derive(Clone)]
pub struct MappingClient {
    client: ClientWithMiddleware,
    compressed_mapping: prefix::CompressedMappingClient,
    hash_mapping: prefix::HashMappingClient,
}

pub struct MappingClientBuilder {
    client: ClientWithMiddleware,
    compressed_mapping: prefix::CompressedMappingClientBuilder,
    hash_mapping: prefix::HashMappingClientBuilder,
}

impl MappingClientBuilder {
    /// Sets the concurrency limit for the client. This is useful to limit the
    /// maximum number of concurrent requests.
    pub fn with_concurrency_limit(self, limit: Arc<Semaphore>) -> Self {
        Self {
            compressed_mapping: self
                .compressed_mapping
                .with_concurrency_limit(limit.clone()),
            hash_mapping: self.hash_mapping.with_concurrency_limit(limit),
            ..self
        }
    }

    /// Sets the concurrency limit for the client. This is useful to limit the
    /// maximum number of concurrent requests.
    pub fn set_concurrency_limit(&mut self, limit: Arc<Semaphore>) -> &mut Self {
        self.compressed_mapping.set_concurrency_limit(limit.clone());
        self.hash_mapping.set_concurrency_limit(limit);
        self
    }

    /// Finish the construction of the client and return it.
    pub fn finish(self) -> MappingClient {
        MappingClient {
            client: self.client,
            compressed_mapping: self.compressed_mapping.finish(),
            hash_mapping: self.hash_mapping.finish(),
        }
    }
}

#[derive(Debug, Error)]
pub enum MappingError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    Reqwest(#[from] reqwest_middleware::Error),
}

impl MappingClient {
    /// Construct a new `MappingClientBuilder` with the provided `Client`.
    pub fn builder(client: ClientWithMiddleware) -> MappingClientBuilder {
        // Construct a client with a retry policy and local caching
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
        let retry_strategy = RetryTransientMiddleware::new_with_policy(retry_policy);
        let cache_strategy = Cache(HttpCache {
            mode: CacheMode::Default,
            manager: CACacheManager {
                path: get_cache_dir()
                    .expect("missing cache directory")
                    .join(pixi_consts::consts::CONDA_PYPI_MAPPING_CACHE_DIR),
            },
            options: HttpCacheOptions::default(),
        });

        let client = ClientBuilder::from_client(client)
            .with(cache_strategy)
            .with(retry_strategy)
            .build();

        MappingClientBuilder {
            client: client.clone(),
            compressed_mapping: prefix::CompressedMappingClient::builder(client.clone()),
            hash_mapping: prefix::HashMappingClient::builder(client),
        }
    }

    /// Given a set of `RepoDataRecord`s, amend the purls for each record.
    pub async fn amend_purls(
        &self,
        mapping_source: &MappingSource,
        conda_packages: impl IntoIterator<Item = &mut RepoDataRecord>,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> miette::Result<()> {
        // Collect the records into a vec so we can iterate multiple times.
        let mut records = conda_packages.into_iter().collect_vec();

        // Normalize the channel names by removing the trailing slash
        for package in records.iter_mut() {
            package.channel = package
                .channel
                .as_ref()
                .map(|c| c.trim_end_matches('/').to_string());
        }

        // Discard all records for which we already have pypi purls.
        records.retain(|record| !has_pypi_purl(record));

        // Fetch custom mapped channels if any.
        let custom_mappings = if let MappingSource::Custom(mapping_url) = mapping_source {
            Some(CustomMappingClient::from(
                mapping_url.fetch_custom_mapping(&self.client).await?,
            ))
        } else {
            None
        };

        let mut amend_futures = FuturesUnordered::new();
        let total_records = records.len();
        for record in records.into_iter() {
            let reporter = reporter.clone();
            let custom_mappings = &custom_mappings;
            let derive_purls_future = async move {
                if let Some(reporter) = reporter.as_deref() {
                    reporter.download_started(record, total_records);
                }

                let derived_purls = if matches!(mapping_source, MappingSource::Disabled) {
                    Ok(None)
                } else if let Some(custom_mappings) = custom_mappings
                    .as_ref()
                    .filter(|mapping| mapping.is_mapping_for_record(record))
                {
                    custom_mappings.derive_purls(record).await
                } else {
                    self.derive_purls_from_clients(record).await
                };

                match derived_purls {
                    Ok(derived_purls) => {
                        if let Some(reporter) = reporter.as_deref() {
                            reporter.download_finished(record, total_records);
                        }
                        Ok((record, derived_purls))
                    }
                    Err(err) => {
                        if let Some(reporter) = reporter.as_deref() {
                            reporter.download_failed(record, total_records);
                        }
                        Err(err)
                    }
                }
            };

            // Add all futures to the futures queue to ensure all can run concurrently.
            amend_futures.push(derive_purls_future);
        }

        while let Some(next) = amend_futures.next().await {
            let (record, mut derived_purls) = next.into_diagnostic()?;

            // As a last resort use the verbatim conda-forge purls.
            if derived_purls.is_none() {
                derived_purls = CondaForgeVerbatim
                    .derive_purls(record)
                    .await
                    .into_diagnostic()?;
            }

            if let Some(derived_purls) = derived_purls {
                amend_purls(record, derived_purls)
            }
        }

        Ok(())
    }

    async fn derive_purls_from_clients(
        &self,
        record: &RepoDataRecord,
    ) -> Result<Option<Vec<PackageUrl>>, MappingError> {
        // Try to get the purls from the hash mapping.
        let mut purls = self.hash_mapping.derive_purls(record).await?;

        // Otherwise try from the compressed mapping
        if purls.is_none() {
            purls = self.compressed_mapping.derive_purls(record).await?;
        }

        Ok(purls)
    }
}

/// Returns true if the record has a pypi purl.
fn has_pypi_purl(record: &RepoDataRecord) -> bool {
    record
        .package_record
        .purls
        .as_ref()
        .is_some_and(|vec| vec.iter().any(|p| p.package_type() == "pypi"))
}

/// Adds the specified purls to the `purls` field of the record.
fn amend_purls(record: &mut RepoDataRecord, purls: impl IntoIterator<Item = PackageUrl>) {
    let record_purls = record
        .package_record
        .purls
        .get_or_insert_with(BTreeSet::new);
    for purl in purls {
        record_purls.insert(purl);
    }
}

/// A trait that is implemented for clients that can derive a purl from a
/// particular record.
trait DerivePurls {
    /// Derives purls from the given record.
    ///
    /// Returns `None` if no purls could be derived. Note that this is different
    /// from `Some(vec[])` which would indicate that purls could be derived but
    /// there were simply none.
    async fn derive_purls(
        &self,
        record: &RepoDataRecord,
    ) -> Result<Option<Vec<PackageUrl>>, MappingError>;
}

/// A struct that provides derived package urls for conda-forge records where
/// the name of the package is just assumed to be the pypi name.
///
/// This is a fallback for when the mapping is not available.
pub struct CondaForgeVerbatim;

impl DerivePurls for CondaForgeVerbatim {
    async fn derive_purls(
        &self,
        record: &RepoDataRecord,
    ) -> Result<Option<Vec<PackageUrl>>, MappingError> {
        if !is_conda_forge_record(record) {
            return Ok(None);
        }

        // Try to convert the name and version into pep440/pep508 compliant versions.
        let (Some(name), Some(_version)) = (
            pep508_rs::PackageName::from_str(record.package_record.name.as_source()).ok(),
            pep440_rs::Version::from_str(&record.package_record.version.as_str()).ok(),
        ) else {
            // If we cannot convert the name or version, we cannot build a purl.
            return Ok(Some(vec![]));
        };

        // Build the purl
        let purl = PackageUrl::builder(String::from("pypi"), name.to_string());
        let built_purl = purl.build().expect("valid pypi package url");
        Ok(Some(vec![built_purl]))
    }
}
