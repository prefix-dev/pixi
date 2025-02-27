//! Defines provider traits for accessing installed Python packages and cached distributions.
//!
//! This module contains two key traits:
//! - `InstalledDistProvider`: Provides iteration over installed Python distributions
//! - `CachedDistProvider`: Provides access to cached package distributions
//!
//! These traits enable abstraction over package installation operations and support
//! mocking for testing purposes. The module implements these traits for concrete types
//! `SitePackages` and `RegistryWheelIndex` respectively.
//!
use uv_distribution::RegistryWheelIndex;
use uv_distribution_types::{CachedRegistryDist, InstalledDist};
use uv_installer::SitePackages;

// Below we define a couple of traits so that we can make the creaton of the install plan
// somewhat more abstract
//
/// Provide an iterator over the installed distributions
/// This trait can also be used to mock the installed distributions for testing purposes
pub trait InstalledDistProvider<'a> {
    /// Provide an iterator over the installed distributions
    fn iter(&'a self) -> impl Iterator<Item = &'a InstalledDist>;
}

impl<'a> InstalledDistProvider<'a> for SitePackages {
    fn iter(&'a self) -> impl Iterator<Item = &'a InstalledDist> {
        self.iter()
    }
}

/// Provides a way to get the potentially cached distribution, if it exists
/// This trait can also be used to mock the cache for testing purposes
pub trait CachedDistProvider<'a> {
    /// Get the cached distribution for a package name and version
    fn get_cached_dist(
        &mut self,
        name: &'a uv_normalize::PackageName,
        version: uv_pep440::Version,
    ) -> Option<CachedRegistryDist>;
}

impl<'a> CachedDistProvider<'a> for RegistryWheelIndex<'a> {
    fn get_cached_dist(
        &mut self,
        name: &'a uv_normalize::PackageName,
        version: uv_pep440::Version,
    ) -> Option<CachedRegistryDist> {
        let index = self
            .get(name)
            .find(|entry| entry.dist.filename.version == version);
        index.map(|index| index.dist.clone())
    }
}
