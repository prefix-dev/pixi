use std::path::PathBuf;

use chrono::Utc;
use itertools::chain;
use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_glob::GlobModificationTime;
use pixi_record::PinnedSourceSpec;
use rattler_conda_types::{ChannelConfig, ChannelUrl};
use tokio::sync::Mutex;
use tracing::instrument;

use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    PackageIdentifier, SourceCheckoutError,
    build::{
        BuildCacheEntry, BuildCacheError, BuildInput, CachedBuild, PackageBuildInputHashBuilder,
    },
};

/// A list of globs that should be ignored when calculating any input hash.
/// These are typically used for build artifacts that should not be included in
/// the input hash.
pub const DEFAULT_BUILD_IGNORE_GLOBS: &[&str] = &["!.pixi/**"];

/// A query to retrieve information from the source build cache. This is
/// memoized to allow querying information from the cache while it is also
/// overwritten at the same time by a build.
///
/// The main use case for this query is to be able to *check* if a given source
/// build _was_ out of date without actually having to build the referenced
/// package.
///
/// There are two ways by which a package is considered outdated.
/// 1. A source file changed.
/// 2. A build dependency changed.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct SourceBuildCacheStatusSpec {
    /// Describes the package to query in the source build cache.
    pub package: PackageIdentifier,

    /// Describes the source location of the package to query.
    pub source: PinnedSourceSpec,

    /// The channels to use when building source packages.
    pub channels: Vec<ChannelUrl>,

    /// The build environment used to build the package.
    pub build_environment: BuildEnvironment,

    /// The channel configuration to use when building the package.
    pub channel_config: ChannelConfig,

    /// The protocols that are enabled when discovering the build backend.
    pub enabled_protocols: EnabledProtocols,
}

pub enum CachedBuildStatus {
    /// The build was found in the cache but is stale.
    Stale(CachedBuild),

    /// The build was found in the cache and is up to date.
    UpToDate(CachedBuild),

    /// The build was not found in the cache.
    Missing,
}

pub struct SourceBuildCacheEntry {
    /// The information stored in the build cache. Or `None` if the build did
    /// not exist in the cache.
    pub cached_build: CachedBuildStatus,

    /// A reference to the build entry in the cache. Not that as long as this
    /// entry exists a lock is retained on the cache entry.
    pub entry: Mutex<BuildCacheEntry>,

    /// The path where the package will be stored.
    pub cache_dir: PathBuf,
}

impl SourceBuildCacheStatusSpec {
    /// Creates a new query for the source build cache.
    #[instrument(skip_all, fields(package = %self.package, source = %self.source))]
    pub async fn query(
        self,
        command_dispatcher: CommandDispatcher,
    ) -> Result<SourceBuildCacheEntry, CommandDispatcherError<SourceBuildCacheStatusError>> {
        // Query the build cache directly.
        let build_input = BuildInput {
            channel_urls: self.channels.clone(),
            name: self.package.name.as_source().to_string(),
            version: self.package.version.to_string(),
            build: self.package.build.to_string(),
            subdir: self.package.subdir.clone(),
            host_platform: self.build_environment.host_platform,
            host_virtual_packages: self.build_environment.host_virtual_packages.clone(),
            build_virtual_packages: self.build_environment.build_virtual_packages.clone(),
        };
        let (cached_build, build_cache_entry) = command_dispatcher
            .build_cache()
            .entry(&self.source, &build_input)
            .await
            .map_err(SourceBuildCacheStatusError::BuildCache)
            .map_err(CommandDispatcherError::Failed)?;

        // Check the staleness of the cached entry
        let cached_build = match cached_build {
            Some(cached_build) => {
                self.determine_cache_status(&command_dispatcher, cached_build)
                    .await?
            }
            None => CachedBuildStatus::Missing,
        };

        Ok(SourceBuildCacheEntry {
            cached_build,
            cache_dir: build_cache_entry.cache_dir().to_path_buf(),
            entry: Mutex::new(build_cache_entry),
        })
    }

    /// Given a cached build, verify that it is still valid for the given source
    /// record.
    async fn determine_cache_status(
        &self,
        command_dispatcher: &CommandDispatcher,
        cached_build: CachedBuild,
    ) -> Result<CachedBuildStatus, CommandDispatcherError<SourceBuildCacheStatusError>> {
        let source = &self.source;

        // Immutable source records are always considered valid.
        if source.is_immutable() {
            return Ok(CachedBuildStatus::UpToDate(cached_build));
        }

        // Check if the project configuration has changed.
        let cached_build = match self
            .check_package_configuration_changed(command_dispatcher, cached_build, source)
            .await?
        {
            CachedBuildStatus::UpToDate(cached_build) => cached_build,
            CachedBuildStatus::Stale(cached_build) => {
                return Ok(CachedBuildStatus::Stale(cached_build));
            }
            CachedBuildStatus::Missing => return Ok(CachedBuildStatus::Missing),
        };

        // Determine if the package is out of date by checking the source
        let cached_build = match self
            .check_source_out_of_date(command_dispatcher, cached_build, source)
            .await?
        {
            CachedBuildStatus::UpToDate(cached_build) => cached_build,
            CachedBuildStatus::Stale(cached_build) => {
                return Ok(CachedBuildStatus::Stale(cached_build));
            }
            CachedBuildStatus::Missing => return Ok(CachedBuildStatus::Missing),
        };

        // Otherwise, check if perhaps any of the build dependencies are out of date
        // which would cause a rebuild.
        self.check_build_dependencies_out_of_date(command_dispatcher, cached_build)
            .await
    }

    /// Checks if any of the build dependencies are out of date.
    ///
    /// A build dependency is considered out of date if:
    /// * The dependency itself is stale.
    /// * The hash of the package that was used during the build does not match
    ///   the current hash in the cache.
    async fn check_build_dependencies_out_of_date(
        &self,
        command_dispatcher: &CommandDispatcher,
        cached_build: CachedBuild,
    ) -> Result<CachedBuildStatus, CommandDispatcherError<SourceBuildCacheStatusError>> {
        let Some(source_info) = &cached_build.source else {
            return Ok(CachedBuildStatus::UpToDate(cached_build));
        };

        // Check if any of the transitive source dependencies have changed.
        for dep in chain!(&source_info.host.packages, &source_info.build.packages) {
            let Some(source) = &dep.source else {
                continue;
            };

            let identifier = PackageIdentifier::from(&dep.repodata_record.package_record);

            // Check the build cache to see if the source of that package is still fresh.
            match command_dispatcher
                .source_build_cache_status(SourceBuildCacheStatusSpec {
                    package: identifier.clone(),
                    source: source.clone(),
                    channels: self.channels.clone(),
                    build_environment: self.build_environment.clone(),
                    channel_config: self.channel_config.clone(),
                    enabled_protocols: self.enabled_protocols.clone(),
                })
                .await
                .try_into_failed()?
            {
                Err(SourceBuildCacheStatusError::Cycle) => {
                    tracing::debug!(
                        "a cycle was detected in the build/host dependencies of the package",
                    );
                    return Ok(CachedBuildStatus::Stale(cached_build));
                }
                Err(err) => {
                    return Err(CommandDispatcherError::Failed(err));
                }
                Ok(entry) => {
                    match &entry.cached_build {
                        CachedBuildStatus::Missing | CachedBuildStatus::Stale(_) => {
                            tracing::debug!(
                                "package is stale because its build dependency '{identifier}' is missing or stale",
                            );
                            return Ok(CachedBuildStatus::Stale(cached_build));
                        }
                        CachedBuildStatus::UpToDate(dependency_cached_build) => {
                            // Is this version of the package also what we expect?
                            //
                            // Maybe the package that we previously used was actually updated
                            // without also updating this package, or the build of this package
                            // failed previously.
                            if dependency_cached_build.record.package_record.sha256
                                != dep.repodata_record.package_record.sha256
                            {
                                tracing::debug!(
                                    "package is stale because its build dependency '{identifier}' has changed",
                                );
                                return Ok(CachedBuildStatus::Stale(cached_build));
                            }
                        }
                    }
                }
            }
        }

        Ok(CachedBuildStatus::UpToDate(cached_build))
    }

    /// Checks if the package configuration has changed by computing a hash and
    /// comparing that against the stored hash.
    ///
    /// TODO: We should optimize this because currently we have to checkout the
    /// source, discover the backend, and compute hashes. We can probably
    /// already early out if some of this information is cached more granularly.
    /// E.g. if the pixi.toml file didnt change (compare using timestamp) then
    /// we can probably skip a bunch of these things.
    async fn check_package_configuration_changed(
        &self,
        command_dispatcher: &CommandDispatcher,
        cached_build: CachedBuild,
        source: &PinnedSourceSpec,
    ) -> Result<CachedBuildStatus, CommandDispatcherError<SourceBuildCacheStatusError>> {
        let Some(source_info) = &cached_build.source else {
            return Ok(CachedBuildStatus::UpToDate(cached_build));
        };

        let Some(current_hash) = source_info.package_build_input_hash else {
            tracing::debug!(
                "package is stale because the package build input hash is missing or stale",
            );
            return Ok(CachedBuildStatus::Stale(cached_build));
        };

        // Checkout the source for the package.
        let source_checkout = command_dispatcher
            .checkout_pinned_source(source.clone())
            .await
            .map_err_with(SourceBuildCacheStatusError::SourceCheckout)?;

        // Determine the backend parameters for the package.
        let backend = command_dispatcher
            .discover_backend(
                &source_checkout.path,
                self.channel_config.clone(),
                self.enabled_protocols.clone(),
            )
            .await
            .map_err_with(SourceBuildCacheStatusError::Discovery)?;

        // Compute a hash of the package configuration.
        let package_build_input_hash = PackageBuildInputHashBuilder {
            project_model: backend.init_params.project_model.as_ref(),
            configuration: backend.init_params.configuration.as_ref(),
            target_configuration: backend.init_params.target_configuration.as_ref(),
        }
        .finish();

        // Compare the hashes
        if current_hash != package_build_input_hash {
            tracing::debug!("package is stale because the package build input hash has changed");
            return Ok(CachedBuildStatus::Stale(cached_build));
        }

        // Compute the input hash of the build.
        Ok(CachedBuildStatus::UpToDate(cached_build))
    }

    /// Returns the status of a cached build by looking at the input files of
    /// the build as returned by the build backend.
    async fn check_source_out_of_date(
        &self,
        command_dispatcher: &CommandDispatcher,
        cached_build: CachedBuild,
        source: &PinnedSourceSpec,
    ) -> Result<CachedBuildStatus, CommandDispatcherError<SourceBuildCacheStatusError>> {
        // If there are no source globs, we always consider the cached package
        // up-to-date.
        let Some(source_info) = cached_build.source.as_ref().filter(|p| !p.globs.is_empty()) else {
            return Ok(CachedBuildStatus::UpToDate(cached_build));
        };

        // Checkout the source for the package.
        let source_checkout = command_dispatcher
            .checkout_pinned_source(source.clone())
            .await
            .map_err_with(SourceBuildCacheStatusError::SourceCheckout)?;

        // Compute the modification time of the files that match the source input globs.
        let glob_time = match GlobModificationTime::from_patterns(
            &source_checkout.path,
            source_info
                .globs
                .iter()
                .map(String::as_str)
                .chain(DEFAULT_BUILD_IGNORE_GLOBS.iter().copied()),
        ) {
            Ok(glob_time) => glob_time,
            Err(e) => {
                tracing::warn!(
                    "failed to determine modification time of input files: {}. Assuming the package is out-of-date.",
                    e
                );
                return Ok(CachedBuildStatus::Stale(cached_build));
            }
        };

        // Determine the staleness of the package based on the timestamps of the last
        // updated file and the package itself.
        match glob_time {
            GlobModificationTime::MatchesFound {
                modified_at,
                designated_file,
            } => {
                if cached_build
                    .record
                    .package_record
                    .timestamp
                    .map(|t| t < chrono::DateTime::<Utc>::from(modified_at))
                    .unwrap_or(true)
                {
                    tracing::debug!(
                        "package is stale, the file {} is newer than the package in cache",
                        designated_file.display()
                    );
                    return Ok(CachedBuildStatus::Stale(cached_build));
                }
            }
            GlobModificationTime::NoMatches => {
                tracing::debug!("package is stale, no files match the source globs",);
                return Ok(CachedBuildStatus::Stale(cached_build));
            }
        }

        Ok(CachedBuildStatus::UpToDate(cached_build))
    }
}

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum SourceBuildCacheStatusError {
    #[error(transparent)]
    BuildCache(BuildCacheError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(SourceCheckoutError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Discovery(pixi_build_discovery::DiscoveryError),

    #[error("a cycle was detected in the build/host dependencies of the package")]
    Cycle,
}
