//! Derive PyPI package URLs for conda packages.
//!
//! There are two related concepts in this crate:
//!
//! - [`PurlDerivationMode`] is the user-selected mapping mode: project-defined, prefix.dev, or disabled.
//! - [`PurlDerivationSource`] is the concrete resolver/provenance for an individual purl.
//!
//! The concrete derivation sources are:
//!
//! 1. [`PurlDerivationSource::ProjectDefinedMapping`] — user/project-defined per-channel mapping.
//! 2. [`PurlDerivationSource::PrefixHashMapping`] — prefix.dev hash mapping by package SHA256.
//! 3. [`PurlDerivationSource::PrefixCompressedMapping`] — prefix.dev compressed name mapping.
//! 4. Same-name heuristic — fallback that assumes the conda package name is
//!    the PyPI package name.
//!
//! A project-defined mapping carries a per-channel [`MappingMode`] that
//! determines how it interacts with Pixi's default mapping data: `Overlay`
//! overlays it (a miss falls through to prefix.dev), `Replace` skips it, and
//! `Disabled` turns derivation for that channel off entirely.

use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{StreamExt, stream::FuturesUnordered};
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use itertools::Itertools;
use rattler_conda_types::{PackageUrl, RepoDataRecord};
use rattler_networking::LazyClient;
use reqwest_middleware::ClientBuilder;
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::Instrument;
mod channel;
mod derivation;
mod derivation_mode;
mod metrics;
mod purl;
mod pypi_names;
mod reporter;
pub mod resolvers;

pub use channel::{is_conda_forge_record, is_conda_forge_url};
pub use derivation_mode::{
    ChannelName, MappingByChannel, MappingMap, MappingMode, ProjectDefinedChannelMapping,
    ProjectDefinedMappingLocation, PurlDerivationMode, ResolvedChannelMapping,
};
pub use metrics::CacheMetrics;
pub use purl::PurlDerivationSource;
pub use pypi_names::PypiNames;
pub use reporter::Reporter;
pub use resolvers::ProjectDefinedMapping;

use crate::{
    derivation::DerivationOutcome,
    resolvers::{ProjectDefined, SameName},
};

/// A compressed mapping maps a conda package name to its PyPI equivalents.
/// An empty [`PypiNames`] means the package is known not to be on PyPI.
pub type CompressedMapping = HashMap<String, PypiNames>;

/// Help text shown when fetching a conda-pypi mapping over the network fails,
/// listing the manifest options that avoid the network lookup.
pub(crate) const MAPPING_OFFLINE_HELP: &str = "If this host cannot be reached (e.g. behind a firewall), you can avoid the network lookup: \
     point the channel's `conda-pypi-map` entry at your own mapping with `location`, \
     replace the default mapping data with `mapping-mode = \"replace\"`, \
     disable the channel with `<channel> = false`, or disable the mapping entirely with \
     `conda-pypi-map = false`.";

/// The mapping client implements the logic to derive purls for conda packages.
///
/// The resolver order depends on [`PurlDerivationMode`]:
///
/// - [`PurlDerivationMode::ProjectDefined`]: project-defined per-channel mapping. How records
///   from a mapped channel interact with the prefix.dev chain depends on the channel's
///   [`MappingMode`]: `Overlay` falls through to prefix.dev on a miss, `Replace` skips
///   prefix.dev mapping data, and `Disabled` skips all derivation. The same-name
///   heuristic is controlled separately per channel. Records from unmapped channels
///   use the prefix.dev chain and the same-name heuristic only for conda-forge.
/// - [`PurlDerivationMode::Prefix`]: prefix hash mapping, then prefix compressed mapping,
///   then the same-name heuristic for conda-forge.
/// - [`PurlDerivationMode::Disabled`]: no project-defined, prefix, or same-name mapping.
///
/// Concrete purl provenance is represented by [`PurlDerivationSource`].
///
/// For more information see:
/// - [`resolvers::PrefixHash`]
/// - [`resolvers::PrefixCompressed`]
/// - the same-name heuristic
#[derive(Clone)]
pub struct PurlDerivationClient {
    client: LazyClient,
    compressed_mapping: resolvers::PrefixCompressed,
    hash_mapping: resolvers::PrefixHash,
    cache_path: PathBuf,
}

pub struct PurlDerivationClientBuilder {
    client: LazyClient,
    compressed_mapping: resolvers::PrefixCompressedBuilder,
    hash_mapping: resolvers::PrefixHashBuilder,
    cache_path: PathBuf,
}

impl PurlDerivationClientBuilder {
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
    pub fn finish(self) -> PurlDerivationClient {
        PurlDerivationClient {
            client: self.client,
            compressed_mapping: self.compressed_mapping.finish(),
            hash_mapping: self.hash_mapping.finish(),
            cache_path: self.cache_path,
        }
    }
}

#[derive(Debug, Error, miette::Diagnostic)]
pub enum MappingError {
    #[error("failed to access conda-pypi mapping cache at '{path}'")]
    IoError {
        #[source]
        source: std::io::Error,
        path: PathBuf,
    },
    #[error("failed to fetch conda-pypi mapping from remote source")]
    #[diagnostic(help("{}", MAPPING_OFFLINE_HELP))]
    Reqwest(#[source] reqwest_middleware::Error),
}

impl From<reqwest_middleware::Error> for MappingError {
    fn from(err: reqwest_middleware::Error) -> Self {
        MappingError::Reqwest(err)
    }
}

impl PurlDerivationClient {
    /// Construct a new `PurlDerivationClientBuilder` with the provided `Client` and
    /// the resolved on-disk `cache_path` for the conda-pypi mapping cache.
    ///
    /// The caller is responsible for resolving `cache_path` (e.g. through
    /// `pixi_config::Config::cache_dir_for`) so that workspace-level
    /// `[cache.pypi-mapping]` overrides are respected; this crate stays
    /// agnostic about which config layer wins.
    pub fn builder(client: LazyClient, cache_path: PathBuf) -> PurlDerivationClientBuilder {
        // Construct a client with a retry policy and local caching
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
        let retry_strategy = RetryTransientMiddleware::new_with_policy(retry_policy);
        let cache_strategy = Cache(HttpCache {
            mode: CacheMode::Default,
            manager: CACacheManager {
                path: cache_path.clone(),
                remove_opts: Default::default(),
            },
            options: HttpCacheOptions::default(),
        });

        let wrapped_client = LazyClient::new(move || {
            let client = client.client().clone();
            ClientBuilder::from_client(client)
                .with(retry_strategy)
                .with(cache_strategy)
                .build()
        });

        PurlDerivationClientBuilder {
            client: wrapped_client.clone(),
            compressed_mapping: resolvers::PrefixCompressed::builder(wrapped_client.clone()),
            hash_mapping: resolvers::PrefixHash::builder(wrapped_client),
            cache_path,
        }
    }

    /// Given a set of `RepoDataRecord`s, amend the purls for each record.
    pub async fn amend_purls(
        &self,
        derivation_mode: &PurlDerivationMode,
        conda_packages: impl IntoIterator<Item = &mut RepoDataRecord>,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> miette::Result<()> {
        let start = Instant::now();

        // Collect the records into a vec so we can iterate multiple times.
        let mut records = conda_packages.into_iter().collect_vec();

        // Normalize the channel names by removing the trailing slash
        for package in records.iter_mut() {
            package.channel = package
                .channel
                .as_ref()
                .map(|c| c.trim_end_matches('/').to_string());
        }

        let metrics = CacheMetrics::default();

        // Fetch project-defined mapped channels if any.
        let project_defined =
            if let PurlDerivationMode::ProjectDefined(mapping_url) = derivation_mode {
                Some(ProjectDefined::from(
                    mapping_url.fetch_project_defined(&self.client).await?,
                ))
            } else {
                None
            };

        let mut amend_futures = FuturesUnordered::new();
        let total_records = records.len();
        for record in records.into_iter() {
            let reporter = reporter.clone();
            let project_defined = &project_defined;
            let cache_metrics = &metrics;
            let file_name = record.identifier.to_file_name();
            let derive_purls_future = async move {
                if let Some(reporter) = reporter.as_deref() {
                    reporter.download_started(record, total_records);
                }

                let derived_purls = self
                    .derive_purls_for_record(
                        derivation_mode,
                        project_defined.as_ref(),
                        record,
                        cache_metrics,
                    )
                    .await;

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
            }
            .instrument(tracing::info_span!("derive_purl", record = file_name));

            // Add all futures to the futures queue to ensure all can run concurrently.
            amend_futures.push(derive_purls_future);
        }

        let mut amended_records = 0;
        let mut total_records = 0;
        while let Some(next) = amend_futures.next().await {
            // Use `Report::new` instead of `into_diagnostic` to preserve the
            // diagnostic help text on `MappingError`.
            let (record, derived_purls) = next.map_err(miette::Report::new)?;

            if let Some(derived_purls) = derived_purls.into_purls() {
                replace_pypi_purls(record, derived_purls);
                amended_records += 1;
            }

            total_records += 1;
        }

        drop(amend_futures);

        let duration = start.elapsed();
        let data = metrics.into_data();
        tracing::info!(
            "Amended {} out of {} records with purls in {:?}. {} cache hits and {} cache misses ({}%).",
            amended_records,
            total_records,
            Duration::from_millis(duration.as_millis() as u64),
            data.cache_hits,
            data.cache_misses,
            if data.cache_hits == 0 && data.cache_misses == 0 {
                100.0
            } else {
                ((data.cache_hits as f64) / ((data.cache_misses + data.cache_hits) as f64)
                    * 10000.0)
                    .round()
                    / 100.0
            },
        );

        Ok(())
    }

    async fn derive_purls_for_record(
        &self,
        derivation_mode: &PurlDerivationMode,
        project_defined: Option<&ProjectDefined>,
        record: &RepoDataRecord,
        cache_metrics: &CacheMetrics,
    ) -> Result<DerivationOutcome, MappingError> {
        /// Secondary lookup sources to consult if the primary lookup has no answer.
        #[derive(Copy, Clone)]
        struct SecondaryLookups {
            /// Consult Pixi's default prefix.dev mapping data.
            prefix: bool,
            /// Consult the offline same-name heuristic.
            same_name: bool,
        }

        let project_defined_behavior = project_defined
            .as_ref()
            .and_then(|mapping| mapping.behavior_for_record(record));

        // Consult the primary source for this record and determine which
        // secondary sources may be consulted when it has no answer.
        let (mut outcome, secondary) = if matches!(derivation_mode, PurlDerivationMode::Disabled) {
            (
                DerivationOutcome::NoPurls,
                SecondaryLookups {
                    prefix: false,
                    same_name: false,
                },
            )
        } else if let (Some(project_defined), Some((mode, same_name))) =
            (project_defined, project_defined_behavior)
        {
            // A hit in the project-defined mapping (including an explicit
            // "not a PyPI package" entry) is always final.
            let project_outcome = match mode {
                MappingMode::Disabled => DerivationOutcome::NoPurls,
                MappingMode::Replace | MappingMode::Overlay => {
                    project_defined
                        .derive_project_defined_purls(record, cache_metrics)
                        .await?
                }
            };
            let secondary = match mode {
                MappingMode::Disabled => SecondaryLookups {
                    prefix: false,
                    same_name: false,
                },
                MappingMode::Replace => SecondaryLookups {
                    prefix: false,
                    same_name,
                },
                MappingMode::Overlay => SecondaryLookups {
                    prefix: true,
                    same_name,
                },
            };
            (project_outcome, secondary)
        } else {
            (
                DerivationOutcome::NotApplicable,
                SecondaryLookups {
                    prefix: true,
                    same_name: is_conda_forge_record(record),
                },
            )
        };

        if outcome.is_not_applicable() && secondary.prefix {
            outcome = self.derive_purls_from_prefix(record, cache_metrics).await?;
        }

        if outcome.is_not_applicable() && secondary.same_name {
            outcome = SameName
                .derive_same_name_purls(record, cache_metrics)
                .await?;
        }

        if outcome.is_not_applicable() && !secondary.prefix && !secondary.same_name {
            outcome = DerivationOutcome::NoPurls;
        }

        Ok(outcome)
    }

    async fn derive_purls_from_prefix(
        &self,
        record: &RepoDataRecord,
        cache_metrics: &CacheMetrics,
    ) -> Result<DerivationOutcome, MappingError> {
        // Try to get the purls from the hash mapping.
        let purls = self
            .hash_mapping
            .derive_prefix_hash_purls(record, cache_metrics)
            .await
            .map_err(|e| self.with_cache_path_context(e))?;

        // Otherwise try from the compressed mapping
        if purls.is_not_applicable() {
            return self
                .compressed_mapping
                .derive_prefix_compressed_purls(record, cache_metrics)
                .await
                .map_err(|e| self.with_cache_path_context(e));
        }

        Ok(purls)
    }

    /// Adds cache path context to a MappingError if it's an IO error.
    fn with_cache_path_context(&self, err: MappingError) -> MappingError {
        match err {
            MappingError::IoError { source, path: _ } => MappingError::IoError {
                source,
                path: self.cache_path.clone(),
            },
            other => other,
        }
    }
}

/// Replaces the PyPI purls in the record with the specified purls.
///
/// Keeping `purls = Some(empty)` is significant: downstream compatibility code
/// treats `None` as "old lock file with unknown purls" and may apply the
/// same-name heuristic. An empty set means "known not to satisfy PyPI names".
fn replace_pypi_purls(record: &mut RepoDataRecord, purls: impl IntoIterator<Item = PackageUrl>) {
    let record_purls = record
        .package_record
        .purls
        .get_or_insert_with(BTreeSet::new);
    record_purls.retain(|purl| purl.package_type() != "pypi");
    record_purls.extend(purls);
}
