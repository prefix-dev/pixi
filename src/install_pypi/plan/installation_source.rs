//! Cache resolution logic for determining whether packages should be installed
//! from local cache or remote registry.
//!
//! This module contains the core logic for deciding installation sources based on:
//! - Cache staleness and revalidation requirements
//! - Local cache availability
//! - Package requirements and versions

use uv_cache::Cache;
use uv_distribution_types::{CachedDist, Dist, Name};

use super::{InstallReason, cache::DistCache};

/// Operation type for determining the appropriate InstallReason
#[derive(Copy, Clone, Debug)]
pub enum Operation {
    /// Installing a new package
    Install,
    /// Reinstalling an existing package
    Reinstall,
}

impl Operation {
    /// Get the InstallReason for when a package is found in cache
    pub fn cached(self) -> InstallReason {
        match self {
            Operation::Install => InstallReason::InstallCached,
            Operation::Reinstall => InstallReason::ReinstallCached,
        }
    }

    /// Get the InstallReason for when a package is stale and needs remote fetch
    pub fn stale(self) -> InstallReason {
        match self {
            Operation::Install => InstallReason::InstallStaleLocal,
            Operation::Reinstall => InstallReason::ReinstallStaleLocal,
        }
    }

    /// Get the InstallReason for when a package is missing from cache
    pub fn missing(self) -> InstallReason {
        match self {
            Operation::Install => InstallReason::InstallMissing,
            Operation::Reinstall => InstallReason::ReinstallMissing,
        }
    }
}

/// Decide if we need to get the distribution from the local cache or the registry
/// this method will add the distribution to the local or remote vector,
/// depending on whether the version is stale, available locally or not
pub fn decide_installation_source<'a>(
    uv_cache: &Cache,
    dist: &'a Dist,
    local: &mut Vec<(CachedDist, InstallReason)>,
    remote: &mut Vec<(Dist, InstallReason)>,
    dist_cache: &mut impl DistCache<'a>,
    operation: Operation,
) -> Result<(), uv_distribution::Error> {
    // First, check if we need to revalidate the package
    // then we should get it from the remote
    if uv_cache.must_revalidate_package(dist.name())
        || dist
            .source_tree()
            .is_some_and(|source_tree| uv_cache.must_revalidate_path(source_tree))
    {
        remote.push((dist.clone(), operation.stale()));
        return Ok(());
    }

    // Check if the distribution is cached
    match dist_cache.is_cached(dist, uv_cache)? {
        Some(cached_dist) => {
            local.push((cached_dist, operation.cached()));
        }
        None => {
            remote.push((dist.clone(), operation.missing()));
        }
    }

    Ok(())
}
