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
use rattler_conda_types::{PackageRecord, PackageUrl, RepoDataRecord};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use thiserror::Error;
use tokio::sync::Semaphore;
use url::Url;

use crate::prefix::{
    CompressedMappingClient, CompressedMappingClientBuilder, HashMappingClient,
    HashMappingClientBuilder, HashMappingClientError,
};

mod custom_mapping;
mod prefix;
mod reporter;

pub use custom_mapping::CustomMapping;
pub use reporter::Reporter;

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

/// Build a purl for a `PackageRecord`
/// it will return a purl in this format
/// `pkg:pypi/aiofiles`
pub fn build_pypi_purl_from_package_record(package_record: &PackageRecord) -> Option<PackageUrl> {
    let name = pep508_rs::PackageName::from_str(package_record.name.as_source()).ok();
    let version = pep440_rs::Version::from_str(&package_record.version.as_str()).ok();
    if let (Some(name), Some(_)) = (name, version) {
        let purl = PackageUrl::builder(String::from("pypi"), name.to_string());
        let built_purl = purl.build().expect("valid pypi package url");
        return Some(built_purl);
    }

    None
}

#[derive(Clone)]
pub struct MappingClient {
    client: ClientWithMiddleware,
    compressed_mapping: CompressedMappingClient,
    hash_mapping: HashMappingClient,
}

pub struct MappingClientBuilder {
    client: ClientWithMiddleware,
    compressed_mapping: CompressedMappingClientBuilder,
    hash_mapping: HashMappingClientBuilder,
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

impl From<HashMappingClientError> for MappingError {
    fn from(value: HashMappingClientError) -> Self {
        match value {
            HashMappingClientError::Io(err) => MappingError::IoError(err),
            HashMappingClientError::Reqwest(err) => MappingError::Reqwest(err),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IsPypiPackage {
    Yes,
    No,
    Unknown,
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
            compressed_mapping: CompressedMappingClient::builder(client.clone()),
            hash_mapping: HashMappingClient::builder(client),
        }
    }

    /// Given a set of `RepoDataRecord`s, amend the purls for each record.
    pub async fn ament_purls(
        &self,
        mapping_source: &MappingSource,
        conda_packages: impl IntoIterator<Item = &mut RepoDataRecord>,
        _reporter: Option<Arc<dyn Reporter>>,
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
            mapping_url.fetch_custom_mapping(&self.client).await?
        } else {
            MappingByChannel::new()
        };

        if !matches!(mapping_source, MappingSource::Disabled) {
            let mut amend_futures = FuturesUnordered::new();
            for record in records.iter_mut() {
                amend_futures.push(async {
                    // Find a custom mapping if available.
                    let custom_mapping = record
                        .channel
                        .as_ref()
                        .and_then(|channel| custom_mappings.get(channel));

                    if let Some(custom_mapping) = custom_mapping {
                        if let Some(possibly_mapped_name) =
                            custom_mapping.get(record.package_record.name.as_normalized())
                        {
                            let purls = record
                                .package_record
                                .purls
                                .get_or_insert_with(BTreeSet::new);

                            if let Some(mapped_name) = possibly_mapped_name {
                                let purl = PackageUrl::builder(
                                    String::from("pypi"),
                                    mapped_name.to_string(),
                                )
                                .with_qualifier(
                                    "source",
                                    PurlSource::ProjectDefinedMapping.as_str(),
                                )
                                .expect("valid qualifier");
                                let built_purl = purl.build().expect("valid pypi package url");
                                purls.insert(built_purl);
                            }
                        }
                    } else {
                        self.ament_purls_from_prefix_clients(record)
                            .await
                            .into_diagnostic()?;
                    };
                    Ok(())
                });
            }

            while let Some(next) = amend_futures.next().await {
                if let Some(err) = next.err() {
                    return Err(err);
                }
            }
        }

        // For the remaining records, if they are conda-forge packages, we just assume
        // that the name is the pypi name.
        for record in records {
            if record.package_record.purls.is_none() && is_conda_forge_record(record) {
                if let Some(purl) = build_pypi_purl_from_package_record(&record.package_record) {
                    record
                        .package_record
                        .purls
                        .get_or_insert_with(BTreeSet::new)
                        .insert(purl);
                }
            }
        }

        Ok(())
    }

    async fn ament_purls_from_prefix_clients(
        &self,
        record: &mut RepoDataRecord,
    ) -> Result<IsPypiPackage, MappingError> {
        // If the record has a sha256, we can use the hash mapping to get the purl.
        if let Some(sha256) = record.package_record.sha256.as_ref() {
            if let Some(mapped) = self.hash_mapping.get_mapping(*sha256).await? {
                let purls = record
                    .package_record
                    .purls
                    .get_or_insert_with(BTreeSet::new);

                if let Some(mapped_name) = mapped.pypi_normalized_names {
                    for pypi_name in mapped_name {
                        let purl = PackageUrl::builder(String::from("pypi"), pypi_name)
                            .with_qualifier("source", PurlSource::HashMapping.as_str())
                            .expect("valid qualifier");
                        let built_purl = purl.build().expect("valid pypi package url");
                        // Push the value into the vector
                        purls.insert(built_purl);
                    }

                    return Ok(IsPypiPackage::Yes);
                } else {
                    return Ok(IsPypiPackage::No);
                }
            }
        }

        // If we dont have a mapping yet, or if the mapping is missing a sha256 hash we
        // try to look up the name in the name mapping.
        if is_conda_forge_record(record) {
            let mapping = self.compressed_mapping.get_mapping().await?;
            if let Some(possible_mapped_name) =
                mapping.get(record.package_record.name.as_normalized())
            {
                let purls = record
                    .package_record
                    .purls
                    .get_or_insert_with(BTreeSet::new);

                // if we have a pypi name for it
                // we record the purl
                if let Some(mapped_name) = possible_mapped_name {
                    let purl = PackageUrl::builder(String::from("pypi"), mapped_name)
                        .with_qualifier("source", PurlSource::CompressedMapping.as_str())
                        .expect("valid qualifier");
                    let built_purl = purl.build().expect("valid pypi package url");
                    purls.insert(built_purl);
                    return Ok(IsPypiPackage::Yes);
                }

                return Ok(IsPypiPackage::No);
            }
        }

        Ok(IsPypiPackage::Unknown)
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
