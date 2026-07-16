//! Cache resolution for distribution packages.
//!
//! This module provides traits and implementations for resolving cached distributions,
//! determining whether packages can be installed from local cache or need to be fetched remotely.
//!
//! The main components are:
//! - `DistCache`: Trait for checking if distributions are cached
//! - `CachedWheelsProvider`: Implementation that checks both registry and built wheel caches
//!
//! NOTE: `CachedWheels::is_cached` mirrors the cache-lookup half of uv's installer plan
//! (`uv-installer/src/plan.rs`, `Planner::build`): pointer formats, freshness checks,
//! and hash enforcement.
//! When bumping uv, diff that function for semantic changes and port them here.

use uv_cache::{CacheBucket, WheelCache};
use uv_cache_info::{CacheInfo, Timestamp};
use uv_configuration::BuildOptions;
use uv_distribution::{BuiltWheelIndex, RegistryWheelIndex};
// uv 0.11.16 renamed `LocalArchivePointer` to `PathArchivePointer` (same API).
use uv_distribution::{HttpArchivePointer, PathArchivePointer};
use uv_distribution_filename::WheelFilename;
use uv_distribution_types::BuiltDist;
use uv_distribution_types::{CachedDirectUrlDist, CachedDist, Dist, Name, SourceDist};
use uv_pypi_types::{HashDigests, VerbatimParsedUrl};
use uv_types::HashStrategy;

#[derive(thiserror::Error, Debug)]
pub enum DistCacheError {
    #[error("URL dependency points to a wheel which conflicts with `--no-binary` option: {url}")]
    NoBinaryConflictUrl { url: String },
    #[error("Path dependency points to a wheel which conflicts with `--no-binary` option: {path}")]
    NoBinaryConflictPath { path: String },
    #[error(transparent)]
    Distribution(#[from] uv_distribution::Error),
}

/// Provides cache lookup functionality for distributions.
/// This trait can also be used to mock the cache for testing purposes.
pub trait DistCache<'a> {
    /// Returns a cached distribution if it exists in the cache.
    /// This method consolidates all cache lookup logic for different distribution types.
    fn is_cached(
        &mut self,
        dist: &'a Dist,
        uv_cache: &uv_cache::Cache,
        build_options: &BuildOptions,
    ) -> Result<Option<CachedDist>, DistCacheError>;
}

/// Provides both access to registry dists and locally built dists
pub struct CachedWheels<'a> {
    registry: RegistryWheelIndex<'a>,
    built: BuiltWheelIndex<'a>,
    /// Both indexes above already filter on this strategy themselves.
    /// Direct URL and path wheels skip the indexes, so the check happens here instead.
    hasher: &'a HashStrategy,
}

impl<'a> CachedWheels<'a> {
    pub fn new(
        registry: RegistryWheelIndex<'a>,
        built: BuiltWheelIndex<'a>,
        hasher: &'a HashStrategy,
    ) -> Self {
        Self {
            registry,
            built,
            hasher,
        }
    }
}

/// Enforces the hash policy on a cached direct URL or path wheel and builds the
/// cached distribution.
/// `None` treats the wheel as not cached, so it is re-fetched and verified.
/// The check only bites once the lock file pins a digest for these artifacts.
/// Today the lock records `hash: None` for direct URL and path wheels.
fn cached_wheel_if_verified(
    dist: &Dist,
    hasher: &HashStrategy,
    filename: WheelFilename,
    url: VerbatimParsedUrl,
    cache_info: CacheInfo,
    hashes: HashDigests,
    path: Box<std::path::Path>,
) -> Option<CachedDist> {
    if !hasher.get(dist).matches(hashes.as_slice()) {
        return None;
    }
    Some(CachedDist::Url(CachedDirectUrlDist {
        filename,
        url,
        hashes,
        cache_info,
        build_info: None,
        path,
    }))
}

impl<'a> DistCache<'a> for CachedWheels<'a> {
    fn is_cached(
        &mut self,
        dist: &'a Dist,
        uv_cache: &uv_cache::Cache,
        build_options: &BuildOptions,
    ) -> Result<Option<CachedDist>, DistCacheError> {
        // Check if installation of a binary version of the package should be allowed.
        // we do not allow to set `no_binary` just yet but let's handle it here
        // because, then this just works
        let no_binary = build_options.no_binary_package(dist.name());
        // We can set no-build
        let no_build = build_options.no_build_package(dist.name());

        match dist {
            Dist::Built(BuiltDist::Registry(wheel)) => {
                // uv 0.11.16 made `RegistryWheelIndex::get` and the `IndexEntry`
                // fields private; `wheel()` is the public replacement with the
                // same index/build-policy/filename matching.
                let cached = self.registry.wheel(wheel, no_build, no_binary);

                if let Some(distribution) = cached {
                    Ok(Some(CachedDist::Registry(distribution.clone())))
                } else {
                    Ok(None)
                }
            }
            Dist::Built(BuiltDist::DirectUrl(wheel)) => {
                if no_binary {
                    return Err(DistCacheError::NoBinaryConflictUrl {
                        url: wheel.url.to_string(),
                    });
                }

                // Find the exact wheel from the cache, since we know the filename in advance.
                let cache_entry = uv_cache
                    .shard(
                        CacheBucket::Wheels,
                        WheelCache::Url(&wheel.url).wheel_dir(wheel.name().as_ref()),
                    )
                    .entry(format!("{}.http", wheel.filename.cache_key()));

                // Read the HTTP pointer.
                match HttpArchivePointer::read_from(&cache_entry) {
                    Ok(Some(pointer)) => {
                        let cache_info = pointer.to_cache_info();
                        let archive = pointer.into_archive();
                        Ok(cached_wheel_if_verified(
                            dist,
                            self.hasher,
                            wheel.filename.clone(),
                            VerbatimParsedUrl {
                                parsed_url: wheel.parsed_url(),
                                verbatim: wheel.url.clone(),
                            },
                            cache_info,
                            archive.hashes,
                            uv_cache.archive(&archive.id).into_boxed_path(),
                        ))
                    }
                    Ok(None) => Ok(None),
                    Err(err) => {
                        tracing::debug!(
                            "failed to deserialize cached URL wheel requirement for: {wheel} ({err})"
                        );
                        Ok(None)
                    }
                }
            }
            Dist::Built(BuiltDist::Path(wheel)) => {
                if no_binary {
                    return Err(DistCacheError::NoBinaryConflictPath {
                        path: wheel.url.to_string(),
                    });
                }

                // Validate that the path exists.
                if !wheel.install_path.exists() {
                    return Ok(None);
                }

                // Find the exact wheel from the cache, since we know the filename in advance.
                let cache_entry = uv_cache
                    .shard(
                        CacheBucket::Wheels,
                        WheelCache::Url(&wheel.url).wheel_dir(wheel.name().as_ref()),
                    )
                    .entry(format!("{}.rev", wheel.filename.cache_key()));

                match PathArchivePointer::read_from(&cache_entry) {
                    Ok(Some(pointer)) => match Timestamp::from_path(&wheel.install_path) {
                        Ok(timestamp) => {
                            if pointer.is_up_to_date(timestamp) {
                                let cache_info = pointer.to_cache_info();
                                let archive = pointer.into_archive();
                                Ok(cached_wheel_if_verified(
                                    dist,
                                    self.hasher,
                                    wheel.filename.clone(),
                                    VerbatimParsedUrl {
                                        parsed_url: wheel.parsed_url(),
                                        verbatim: wheel.url.clone(),
                                    },
                                    cache_info,
                                    archive.hashes,
                                    uv_cache.archive(&archive.id).into_boxed_path(),
                                ))
                            } else {
                                Ok(None)
                            }
                        }
                        Err(err) => {
                            tracing::debug!("failed to get timestamp for wheel {wheel} ({err})");
                            Ok(None)
                        }
                    },
                    Ok(None) => Ok(None),
                    Err(err) => {
                        tracing::debug!(
                            "failed to deserialize cached path wheel requirement for: {wheel} ({err})"
                        );
                        Ok(None)
                    }
                }
            }
            Dist::Built(BuiltDist::GitPath(_)) => {
                // uv split the Git source type into GitDirectory + GitPath in
                // 0.11.16; pixi never produces git-archive built distributions.
                // Treat as not cached so it falls through to a normal fetch.
                Ok(None)
            }
            Dist::Source(source_dist) => {
                match source_dist {
                    SourceDist::Path(p) if !p.install_path.exists() => return Ok(None),
                    SourceDist::Directory(p) if !p.install_path.exists() => return Ok(None),
                    _ => {}
                }
                match source_dist {
                    SourceDist::Registry(sdist) => {
                        // uv 0.11.16 made `RegistryWheelIndex::get` and the
                        // `IndexEntry` fields private; `source()` is the public
                        // replacement with the same index/build-policy/name/version
                        // matching.
                        let cached = self.registry.source(sdist, no_build, no_binary);

                        if let Some(distribution) = cached {
                            Ok(Some(CachedDist::Registry(distribution.clone())))
                        } else {
                            Ok(None)
                        }
                    }
                    _ => {
                        let dist = match &source_dist {
                            SourceDist::Directory(directory_source_dist) => self
                                .built
                                .directory(directory_source_dist)?
                                .map(|dist| dist.into_directory_dist(directory_source_dist)),

                            SourceDist::DirectUrl(direct_url_source_dist) => self
                                .built
                                .url(direct_url_source_dist)?
                                .map(|dist| dist.into_url_dist(direct_url_source_dist)),

                            SourceDist::GitDirectory(git_source_dist) => self
                                .built
                                .git_directory(git_source_dist)
                                .map(|dist| dist.into_git_dist(git_source_dist)),

                            SourceDist::Path(path_source_dist) => self
                                .built
                                .path(path_source_dist)?
                                .map(|dist| dist.into_path_dist(path_source_dist)),

                            SourceDist::GitPath(_) => {
                                // pixi never produces git-archive source dists
                                // (uv's 0.11.16 GitDirectory/GitPath split).
                                None
                            }
                            SourceDist::Registry(_) => {
                                unreachable!("handled above")
                            }
                        };
                        if let Some(dist) = dist {
                            if &dist.filename.name == source_dist.name() {
                                Ok(Some(CachedDist::Url(dist)))
                            } else {
                                Ok(None)
                            }
                        } else {
                            Ok(None)
                        }
                    }
                }
            }
        }
    }
}
