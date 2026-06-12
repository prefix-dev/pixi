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
//! 4. [`PurlDerivationSource::CondaForgeVerbatimFallback`] — conda-forge fallback that assumes
//!    the conda package name is the PyPI package name.
//!
//! A project-defined mapping carries a per-channel [`MappingMode`] that
//! determines how it interacts with the prefix.dev chain: `Extend` overlays
//! it (a miss falls through to prefix.dev), `Replace` is exclusive, and
//! `Disabled` turns lookups for that channel off entirely.

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
    resolvers::{CondaForgeVerbatim, ProjectDefinedResolver},
};

/// A compressed mapping is a mapping of a package name to a potential pypi
/// name.
pub type CompressedMapping = HashMap<String, Option<String>>;

/// Help text shown when fetching a conda-pypi mapping over the network fails,
/// listing the manifest options that avoid the network lookup.
pub(crate) const MAPPING_OFFLINE_HELP: &str = "If this host cannot be reached (e.g. behind a firewall), you can avoid the network lookup: \
     point the channel's `conda-pypi-map` entry at your own mapping with `location` (optionally \
     cached with `cache-ttl`), make it exclusive with `mode = \"replace\"`, disable the channel \
     with `<channel> = false`, or disable the mapping entirely with `conda-pypi-map = false`.";

/// The mapping client implements the logic to derive purls for conda packages.
///
/// The resolver order depends on [`PurlDerivationMode`]:
///
/// - [`PurlDerivationMode::ProjectDefined`]: project-defined per-channel mapping. How records
///   from a mapped channel interact with the prefix.dev chain depends on the channel's
///   [`MappingMode`]: `Extend` falls through to prefix.dev on a miss, `Replace` is exclusive
///   (no prefix.dev, no verbatim fallback), and `Disabled` skips all lookups. Records from
///   unmapped channels use the prefix.dev chain.
/// - [`PurlDerivationMode::Prefix`]: prefix hash mapping, then prefix compressed mapping,
///   then the conda-forge verbatim fallback.
/// - [`PurlDerivationMode::Disabled`]: no project-defined or prefix mapping. The current behavior
///   still allows the conda-forge verbatim fallback.
///
/// Concrete purl provenance is represented by [`PurlDerivationSource`].
///
/// For more information see:
/// - [`resolvers::PrefixHashResolver`]
/// - [`resolvers::PrefixCompressedResolver`]
/// - [`PurlDerivationSource::CondaForgeVerbatimFallback`]
#[derive(Clone)]
pub struct PurlDerivationClient {
    client: LazyClient,
    compressed_mapping: resolvers::PrefixCompressedResolver,
    hash_mapping: resolvers::PrefixHashResolver,
    cache_path: PathBuf,
}

pub struct PurlDerivationClientBuilder {
    client: LazyClient,
    compressed_mapping: resolvers::PrefixCompressedResolverBuilder,
    hash_mapping: resolvers::PrefixHashResolverBuilder,
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
            compressed_mapping: resolvers::PrefixCompressedResolver::builder(
                wrapped_client.clone(),
            ),
            hash_mapping: resolvers::PrefixHashResolver::builder(wrapped_client),
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

        // Discard all records for which we already have pypi purls.
        records.retain(|record| !has_pypi_purl(record));

        let metrics = CacheMetrics::default();

        // Fetch project-defined mapped channels if any.
        let project_defined_mappings =
            if let PurlDerivationMode::ProjectDefined(mapping_url) = derivation_mode {
                Some(ProjectDefinedResolver::from(
                    mapping_url
                        .fetch_project_defined_mapping(&self.client, &self.cache_path)
                        .await?,
                ))
            } else {
                None
            };

        let mut amend_futures = FuturesUnordered::new();
        let total_records = records.len();
        for record in records.into_iter() {
            let reporter = reporter.clone();
            let project_defined_mappings = &project_defined_mappings;
            let cache_metrics = &metrics;
            let file_name = record.identifier.to_file_name();
            let derive_purls_future = async move {
                if let Some(reporter) = reporter.as_deref() {
                    reporter.download_started(record, total_records);
                }

                let derived_purls = self
                    .derive_purls_for_record(
                        derivation_mode,
                        project_defined_mappings.as_ref(),
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
                amend_purls(record, derived_purls);
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
        project_defined_mappings: Option<&ProjectDefinedResolver>,
        record: &RepoDataRecord,
        cache_metrics: &CacheMetrics,
    ) -> Result<DerivationOutcome, MappingError> {
        /// What is consulted when the primary lookup does not apply to a record.
        enum Fallback {
            /// The prefix.dev chain, then the offline conda-forge verbatim
            /// heuristic (assume the conda name is the PyPI name).
            PrefixThenVerbatim,
            /// Only the offline conda-forge verbatim heuristic.
            Verbatim,
            /// Nothing: a miss means the record gets no purls. Used for
            /// `Replace` mappings, which are exclusive.
            None,
        }

        let project_defined_mode = project_defined_mappings
            .as_ref()
            .and_then(|mapping| mapping.mode_for_record(record));

        // Consult the primary source for this record and determine which
        // fallback applies when it has no answer.
        let (mut outcome, fallback) = if matches!(derivation_mode, PurlDerivationMode::Disabled) {
            (DerivationOutcome::NotApplicable, Fallback::Verbatim)
        } else if let (Some(resolver), Some(mode)) =
            (project_defined_mappings, project_defined_mode)
        {
            // A hit in the project-defined mapping (including an explicit
            // "not a PyPI package" entry) is always final.
            let project_outcome = match mode {
                MappingMode::Disabled => DerivationOutcome::NotApplicable,
                MappingMode::Replace | MappingMode::Extend => {
                    resolver
                        .derive_project_defined_purls(record, cache_metrics)
                        .await?
                }
            };
            let fallback = match mode {
                MappingMode::Disabled => Fallback::Verbatim,
                MappingMode::Replace => Fallback::None,
                MappingMode::Extend => Fallback::PrefixThenVerbatim,
            };
            (project_outcome, fallback)
        } else {
            (
                DerivationOutcome::NotApplicable,
                Fallback::PrefixThenVerbatim,
            )
        };

        if outcome.is_not_applicable() {
            outcome = match fallback {
                Fallback::PrefixThenVerbatim => {
                    let prefix_outcome =
                        self.derive_purls_from_prefix(record, cache_metrics).await?;
                    if prefix_outcome.is_not_applicable() {
                        CondaForgeVerbatim
                            .derive_conda_forge_verbatim_purls(record, cache_metrics)
                            .await?
                    } else {
                        prefix_outcome
                    }
                }
                Fallback::Verbatim => {
                    CondaForgeVerbatim
                        .derive_conda_forge_verbatim_purls(record, cache_metrics)
                        .await?
                }
                Fallback::None => outcome,
            };
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
