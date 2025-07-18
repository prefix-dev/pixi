use std::path::PathBuf;
use tokio::sync::Mutex;

use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    PackageIdentifier, SourceCheckoutError,
    build::{BuildCacheEntry, BuildCacheError, BuildInput, CachedBuild},
};
use chrono::Utc;
use miette::Diagnostic;
use pixi_glob::{GlobModificationTime, GlobModificationTimeError};
use pixi_record::PinnedSourceSpec;
use rattler_conda_types::ChannelUrl;
use thiserror::Error;

/// A query to retrieve information from the source build cache. This is
/// memoized to allow querying information from the cache while it is also
/// overwritten at the same time by a build.
///
/// The main use case for this query is to be able to check if a given source
/// build _was_ out of date or not without actually having to build to
/// referenced package.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct QuerySourceBuildCache {
    /// Describes the package to query in the source build cache.
    pub package: PackageIdentifier,

    /// Describes the source location of the package to query.
    pub source: PinnedSourceSpec,

    /// The channels to use when building source packages.
    pub channels: Vec<ChannelUrl>,

    /// The build environment used to build the package.
    pub build_environment: BuildEnvironment,
}

#[derive(Debug, Error)]
pub enum StaleReason {
    #[error("failed to determine modification time of input files: {0}")]
    GlobError(GlobModificationTimeError),

    #[error("no files match the source glob")]
    NoMatches,

    #[error("the file {} is newer than the package in cache", .0.display())]
    Timestamp(PathBuf),
}

pub enum CachedBuildStatus {
    /// The build was found in the cache but is stale.
    Stale(CachedBuild, StaleReason),

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

impl QuerySourceBuildCache {
    /// Creates a new query for the source build cache.
    pub async fn query(
        self,
        command_dispatcher: CommandDispatcher,
    ) -> Result<SourceBuildCacheEntry, CommandDispatcherError<QuerySourceBuildCacheError>> {
        let build_input = BuildInput {
            channel_urls: self.channels.clone(),
            name: self.package.name.as_source().to_string(),
            version: self.package.version.to_string(),
            build: self.package.build.to_string(),
            subdir: self.package.subdir.clone(),
            host_platform: self.build_environment.host_platform,
            host_virtual_packages: self.build_environment.host_virtual_packages,
            build_virtual_packages: self.build_environment.build_virtual_packages,
        };
        let (cached_build, build_cache_entry) = command_dispatcher
            .build_cache()
            .entry(&self.source, &build_input)
            .await
            .map_err(QuerySourceBuildCacheError::BuildCache)
            .map_err(CommandDispatcherError::Failed)?;

        Ok(SourceBuildCacheEntry {
            cached_build: match cached_build {
                Some(cached_build) => {
                    Self::determine_cache_status(&command_dispatcher, cached_build, &self.source)
                        .await?
                }
                None => CachedBuildStatus::Missing,
            },
            cache_dir: build_cache_entry.cache_dir().to_path_buf(),
            entry: Mutex::new(build_cache_entry),
        })
    }

    /// Given a cached build, verify that it is still valid for the given source
    /// record.
    async fn determine_cache_status(
        command_dispatcher: &CommandDispatcher,
        cached_build: CachedBuild,
        source: &PinnedSourceSpec,
    ) -> Result<CachedBuildStatus, CommandDispatcherError<QuerySourceBuildCacheError>> {
        // Immutable source records are always considered valid.
        if source.is_immutable() {
            return Ok(CachedBuildStatus::UpToDate(cached_build));
        }

        // If there are no source globs, we always consider the cached package
        // up-to-date.
        let Some(source_info) = cached_build.source.as_ref().filter(|p| !p.globs.is_empty()) else {
            return Ok(CachedBuildStatus::UpToDate(cached_build));
        };

        // Checkout the source for the package.
        let source_checkout = command_dispatcher
            .checkout_pinned_source(source.clone())
            .await
            .map_err_with(QuerySourceBuildCacheError::SourceCheckout)?;

        // Compute the modification time of the files that match the source input globs.
        let glob_time = match GlobModificationTime::from_patterns(
            &source_checkout.path,
            source_info.globs.iter().map(String::as_str).chain(
                crate::install_pixi::DEFAULT_BUILD_IGNORE_GLOBS
                    .iter()
                    .copied(),
            ),
        ) {
            Ok(glob_time) => glob_time,
            Err(e) => {
                tracing::warn!(
                    "failed to determine modification time of input files: {}. Assuming the package is out-of-date.",
                    e
                );
                return Ok(CachedBuildStatus::Stale(
                    cached_build,
                    StaleReason::GlobError(e),
                ));
            }
        };

        match glob_time {
            GlobModificationTime::MatchesFound {
                modified_at,
                designated_file,
            } => {
                if cached_build
                    .record
                    .package_record
                    .timestamp
                    .map(|t| t >= chrono::DateTime::<Utc>::from(modified_at))
                    .unwrap_or(false)
                {
                    Ok(CachedBuildStatus::UpToDate(cached_build))
                } else {
                    Ok(CachedBuildStatus::Stale(
                        cached_build,
                        StaleReason::Timestamp(designated_file),
                    ))
                }
            }
            GlobModificationTime::NoMatches => {
                // No matches, so we should rebuild.
                Ok(CachedBuildStatus::Stale(
                    cached_build,
                    StaleReason::NoMatches,
                ))
            }
        }
    }
}

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum QuerySourceBuildCacheError {
    #[error(transparent)]
    BuildCache(BuildCacheError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(SourceCheckoutError),
}
