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
            Operation::Install => InstallReason::InstallStaleCached,
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

/// Specifies the sources from which distributions can be installed.
/// It contains references to both local cached distributions and remote distributions
pub struct InstallationSources {
    /// These distributions are available in the local cache
    pub cached: Vec<(CachedDist, InstallReason)>,
    /// These distributions are not available in the local cache and must be fetched from the remote registry
    pub remote: Vec<(Dist, InstallReason)>,
}

impl InstallationSources {
    pub fn new() -> Self {
        Self {
            cached: Vec::new(),
            remote: Vec::new(),
        }
    }

    pub fn add_cached(&mut self, cached_dist: &CachedDist, reason: InstallReason) {
        self.cached.push((cached_dist.clone(), reason));
    }

    pub fn add_remote(&mut self, dist: &Dist, reason: InstallReason) {
        self.remote.push((dist.clone(), reason));
    }
}

/// Decide if we need to get the distribution from the local cache or the registry
/// this method will add the distribution to the local or remote vector,
/// depending on whether the version is stale, available locally or not
pub fn decide_installation_source<'a>(
    uv_cache: &Cache,
    dist: &'a Dist,
    dist_cache: &mut impl DistCache<'a>,
    operation: Operation,
) -> Result<InstallationSources, uv_distribution::Error> {
    let mut installation_sources = InstallationSources::new();
    // First, check if we need to revalidate the package
    // then we should get it from the remote
    if uv_cache.must_revalidate_package(dist.name())
        || dist
            .source_tree()
            .is_some_and(|source_tree| uv_cache.must_revalidate_path(source_tree))
    {
        installation_sources.add_remote(dist, operation.stale());
        return Ok(installation_sources);
    }

    // Check if the distribution is cached
    match dist_cache.is_cached(dist, uv_cache)? {
        Some(cached_dist) => {
            installation_sources.add_cached(&cached_dist, operation.cached());
        }
        None => {
            installation_sources.add_remote(dist, operation.missing());
        }
    }

    Ok(installation_sources)
}
