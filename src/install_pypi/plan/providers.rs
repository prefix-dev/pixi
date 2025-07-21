//! Defines provider traits for accessing installed Python packages and cached distributions.
//!
//! This module contains two key traits:
//! - `InstalledDistProvider`: Provides iteration over installed Python distributions
//! - `CachedDistProvider`: Provides access to cached package distributions
//!
//! These traits enable abstraction over package installation operations and support
//! mocking for testing purposes. The module implements these traits for concrete types
//! `SitePackages` and `RegistryWheelIndex`, `BuiltWheelIndex` respectively.
//!
use uv_distribution::{BuiltWheelIndex, RegistryWheelIndex};
use uv_distribution_filename::WheelFilename;
use uv_distribution_types::{
    CachedDirectUrlDist, CachedDist, CachedRegistryDist, IndexUrl, InstalledDist, Name, SourceDist,
};
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
    /// Returns a cached distribution for a registry distribution with the given name, index, and wheel filename.
    fn get_cached_registry_dist(
        &mut self,
        name: &'a uv_normalize::PackageName,
        index: &IndexUrl,
        wheel_filename: &WheelFilename,
    ) -> Option<CachedRegistryDist>;

    /// Returns a cached distribution for a source distribution with the given name, index, and version.
    fn get_cached_registry_source_dist(
        &mut self,
        name: &'a uv_normalize::PackageName,
        index: &IndexUrl,
        version: &uv_pep440::Version,
    ) -> Option<CachedRegistryDist>;

    /// Returns a cached distribution for a source distribution.
    fn get_cached_source_dist(
        &mut self,
        source_dist: &SourceDist,
    ) -> Result<Option<CachedDirectUrlDist>, uv_distribution::Error>;
}

/// Provides both access to registry dists and locally built dists
pub struct CachedWheelsProvider<'a> {
    registry: RegistryWheelIndex<'a>,
    built: BuiltWheelIndex<'a>,
}

impl<'a> CachedWheelsProvider<'a> {
    pub fn new(registry: RegistryWheelIndex<'a>, built: BuiltWheelIndex<'a>) -> Self {
        Self { registry, built }
    }
}

impl<'a> CachedDistProvider<'a> for CachedWheelsProvider<'a> {
    fn get_cached_registry_dist(
        &mut self,
        name: &'a uv_normalize::PackageName,
        index: &IndexUrl,
        wheel_filename: &WheelFilename,
    ) -> Option<CachedRegistryDist> {
        self.registry.get(name).find_map(|entry| {
            if entry.index.url() != index {
                return None;
            }
            if entry.dist.filename == *wheel_filename {
                return None;
            }
            Some(&entry.dist).cloned()
        })
    }

    fn get_cached_registry_source_dist(
        &mut self,
        name: &'a uv_normalize::PackageName,
        index: &IndexUrl,
        version: &uv_pep440::Version,
    ) -> Option<CachedRegistryDist> {
        self.registry.get(name).find_map(|entry| {
            if entry.index.url() != index {
                return None;
            }
            if entry.dist.filename.name != *name {
                return None;
            }
            if entry.dist.filename.version != *version {
                return None;
            }
            Some(&entry.dist).cloned()
        })
    }

    fn get_cached_source_dist(
        &mut self,
        source_dist: &SourceDist,
    ) -> Result<Option<CachedDirectUrlDist>, uv_distribution::Error> {
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
                unimplemented!("call get_cached_registry_source_dist instead")
            }
        };
        if let Some(dist) = dist {
            if &dist.filename.name == source_dist.name() {
                Ok(Some(dist))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }
}
