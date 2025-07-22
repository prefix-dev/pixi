//! Cache resolution logic for determining whether packages should be installed
//! from local cache or remote registry.
//!
//! This module contains the core logic for deciding installation sources based on:
//! - Cache staleness and revalidation requirements
//! - Local cache availability
//! - Package requirements and versions

use uv_cache::{Cache, CacheBucket, WheelCache};
use uv_cache_info::Timestamp;
use uv_distribution::{HttpArchivePointer, LocalArchivePointer};
use uv_distribution_types::{BuiltDist, CachedDirectUrlDist, CachedDist, Dist, Name, SourceDist};
use uv_pypi_types::VerbatimParsedUrl;
use uv_redacted::DisplaySafeUrl;

use crate::install_pypi::conversions::ConvertToUvDistError;

use super::{InstallReason, providers::CachedDistProvider, reasons::OperationToReason};

#[derive(thiserror::Error, Debug)]
pub enum CacheResolverError {
    #[error(transparent)]
    ConvertToUvDist(#[from] ConvertToUvDistError),
    #[error(transparent)]
    UvConversion(#[from] pixi_uv_conversions::ConversionError),
    #[error("the distribution could not be found at the specified path: {0}")]
    NotFound(DisplaySafeUrl),
}

/// Decide if we need to get the distribution from the local cache or the registry
/// this method will add the distribution to the local or remote vector,
/// depending on whether the version is stale, available locally or not
pub fn decide_installation_source<'a, Op: OperationToReason>(
    uv_cache: &Cache,
    dist: &'a Dist,
    local: &mut Vec<(CachedDist, InstallReason)>,
    remote: &mut Vec<(Dist, InstallReason)>,
    dist_cache: &mut impl CachedDistProvider<'a>,
    op_to_reason: Op,
) -> Result<(), CacheResolverError> {
    // First, check if we need to revalidate the package
    // then we should get it from the remote
    if uv_cache.must_revalidate_package(dist.name())
        || dist
            .source_tree()
            .is_some_and(|source_tree| uv_cache.must_revalidate_path(source_tree))
    {
        remote.push((dist.clone(), op_to_reason.stale()));
        return Ok(());
    }

    match dist {
        Dist::Built(BuiltDist::Registry(wheel)) => {
            if let Some(distribution) = dist_cache.get_cached_registry_dist(
                wheel.name(),
                &wheel.best_wheel().index,
                &wheel.best_wheel().filename,
            ) {
                local.push((
                    CachedDist::Registry(distribution.clone()),
                    op_to_reason.cached(),
                ));
                return Ok(());
            }
        }
        Dist::Built(BuiltDist::DirectUrl(wheel)) => {
            // Find the exact wheel from the cache, since we know the filename in
            // advance.
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

                    local.push((CachedDist::Url(cached_dist), op_to_reason.cached()));
                    return Ok(());
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::debug!(
                        "failed to deserialize cached URL wheel requirement for: {wheel} ({err})"
                    );
                }
            }
        }
        Dist::Built(BuiltDist::Path(wheel)) => {
            // Validate that the path exists.
            if !wheel.install_path.exists() {
                return Err(CacheResolverError::NotFound(wheel.url.to_url()));
            }

            // Find the exact wheel from the cache, since we know the filename in
            // advance.
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

                            local.push((CachedDist::Url(cached_dist), op_to_reason.cached()));
                            return Ok(());
                        }
                    }
                    Err(err) => {
                        tracing::debug!("failed to get timestamp for wheel {wheel} ({err})");
                    }
                },
                Ok(None) => {}
                Err(err) => {
                    tracing::debug!(
                        "failed to deserialize cached path wheel requirement for: {wheel} ({err})"
                    );
                }
            }
        }
        Dist::Source(source_dist) => {
            match source_dist {
                SourceDist::Path(p) => {
                    // Validate that the path exists.
                    if !p.install_path.exists() {
                        return Err(CacheResolverError::NotFound(p.url.to_url()));
                    }
                }
                SourceDist::Directory(p) => {
                    // Validate that the path exists.
                    if !p.install_path.exists() {
                        return Err(CacheResolverError::NotFound(p.url.to_url()));
                    }
                }
                _ => {}
            }
            match source_dist {
                SourceDist::Registry(sdist) => {
                    if let Some(distribution) = dist_cache.get_cached_registry_source_dist(
                        sdist.name(),
                        &sdist.index,
                        &sdist.version,
                    ) {
                        local.push((
                            CachedDist::Registry(distribution.clone()),
                            op_to_reason.cached(),
                        ));
                        return Ok(());
                    }
                }
                _ => match dist_cache.get_cached_source_dist(&source_dist) {
                    Ok(cached_dist) => {
                        if let Some(cached) = cached_dist {
                            local.push((CachedDist::Url(cached), op_to_reason.cached()));
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        tracing::warn!("failed to deserialize cached source dist {e}")
                    }
                },
            }
        }
    }

    // If we reach here, it means we didn't find the distribution in the local cache
    remote.push((dist.clone(), op_to_reason.missing()));
    Ok(())
}
