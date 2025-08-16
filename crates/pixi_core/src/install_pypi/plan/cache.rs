//! Cache resolution for distribution packages.
//!
//! This module provides traits and implementations for resolving cached distributions,
//! determining whether packages can be installed from local cache or need to be fetched remotely.
//!
//! The main components are:
//! - `DistCache`: Trait for checking if distributions are cached
//! - `CachedWheelsProvider`: Implementation that checks both registry and built wheel caches

use uv_cache::{CacheBucket, WheelCache};
use uv_cache_info::Timestamp;
use uv_configuration::BuildOptions;
use uv_distribution::{BuiltWheelIndex, RegistryWheelIndex};
use uv_distribution::{HttpArchivePointer, LocalArchivePointer};
use uv_distribution_types::BuiltDist;
use uv_distribution_types::{CachedDirectUrlDist, CachedDist, Dist, Name, SourceDist};
use uv_pypi_types::VerbatimParsedUrl;

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
}

impl<'a> CachedWheels<'a> {
    pub fn new(registry: RegistryWheelIndex<'a>, built: BuiltWheelIndex<'a>) -> Self {
        Self { registry, built }
    }
}

impl<'a> DistCache<'a> for CachedWheels<'a> {
    fn is_cached(
        &mut self,
        dist: &'a Dist,
        uv_cache: &uv_cache::Cache,
        build_options: &BuildOptions,
    ) -> Result<Option<CachedDist>, DistCacheError> {
        // Check if installation of a binary version of the package should be allowed.
        // we do not allow to set `no_binary` just yet but lets handle it here
        // because, then this just works
        let no_binary = build_options.no_binary_package(dist.name());
        // We can set no-build
        let no_build = build_options.no_build_package(dist.name());

        match dist {
            Dist::Built(BuiltDist::Registry(wheel)) => {
                let cached = self.registry.get(wheel.name()).find_map(|entry| {
                    if entry.index.url() != &wheel.best_wheel().index {
                        return None;
                    }
                    if entry.built && no_build {
                        return None;
                    }
                    if !entry.built && no_binary {
                        return None;
                    }
                    if entry.dist.filename == wheel.best_wheel().filename {
                        Some(&entry.dist)
                    } else {
                        None
                    }
                });

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
                        let cached_dist = CachedDirectUrlDist {
                            filename: wheel.filename.clone(),
                            url: VerbatimParsedUrl {
                                parsed_url: wheel.parsed_url(),
                                verbatim: wheel.url.clone(),
                            },
                            hashes: archive.hashes,
                            cache_info,
                            path: uv_cache.archive(&archive.id).into_boxed_path(),
                        };

                        Ok(Some(CachedDist::Url(cached_dist)))
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

                match LocalArchivePointer::read_from(&cache_entry) {
                    Ok(Some(pointer)) => match Timestamp::from_path(&wheel.install_path) {
                        Ok(timestamp) => {
                            if pointer.is_up_to_date(timestamp) {
                                let cache_info = pointer.to_cache_info();
                                let archive = pointer.into_archive();
                                let cached_dist = CachedDirectUrlDist {
                                    filename: wheel.filename.clone(),
                                    url: VerbatimParsedUrl {
                                        parsed_url: wheel.parsed_url(),
                                        verbatim: wheel.url.clone(),
                                    },
                                    hashes: archive.hashes,
                                    cache_info,
                                    path: uv_cache.archive(&archive.id).into_boxed_path(),
                                };

                                Ok(Some(CachedDist::Url(cached_dist)))
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
            Dist::Source(source_dist) => {
                match source_dist {
                    SourceDist::Path(p) => {
                        // Validate that the path exists.
                        if !p.install_path.exists() {
                            return Ok(None);
                        }
                    }
                    SourceDist::Directory(p) => {
                        // Validate that the path exists.
                        if !p.install_path.exists() {
                            return Ok(None);
                        }
                    }
                    _ => {}
                }
                match source_dist {
                    SourceDist::Registry(sdist) => {
                        let cached = self.registry.get(sdist.name()).find_map(|entry| {
                            if entry.index.url() != &sdist.index {
                                return None;
                            }
                            if entry.dist.filename.name != *sdist.name() {
                                return None;
                            }
                            if entry.built && no_build {
                                return None;
                            }
                            if !entry.built && no_binary {
                                return None;
                            }
                            if entry.dist.filename.version == sdist.version {
                                Some(&entry.dist)
                            } else {
                                None
                            }
                        });

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

                            SourceDist::Git(git_source_dist) => self
                                .built
                                .git(git_source_dist)
                                .map(|dist| dist.into_git_dist(git_source_dist)),

                            SourceDist::Path(path_source_dist) => self
                                .built
                                .path(path_source_dist)?
                                .map(|dist| dist.into_path_dist(path_source_dist)),

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
