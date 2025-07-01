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
use uv_pypi_types::{HashAlgorithm, HashDigest};

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
        expected_hash: Option<&rattler_lock::PackageHashes>,
    ) -> Option<CachedRegistryDist>;
}

/// Check if a hash digest matches the expected package hash
fn hash_matches_expected(
    hash: &HashDigest,
    algorithm: HashAlgorithm,
    expected: &rattler_lock::PackageHashes,
) -> bool {
    match (expected, algorithm) {
        (rattler_lock::PackageHashes::Sha256(expected_sha256), HashAlgorithm::Sha256) => {
            format!("{:x}", expected_sha256) == hash.to_string()
        }
        (rattler_lock::PackageHashes::Md5(expected_md5), HashAlgorithm::Md5) => {
            format!("{:x}", expected_md5) == hash.to_string()
        }
        (rattler_lock::PackageHashes::Md5Sha256(expected_md5, expected_sha256), algo) => match algo
        {
            HashAlgorithm::Sha256 => format!("{:x}", expected_sha256) == hash.to_string(),
            HashAlgorithm::Md5 => format!("{:x}", expected_md5) == hash.to_string(),
            _ => false,
        },
        _ => false,
    }
}

impl<'a> CachedDistProvider<'a> for RegistryWheelIndex<'a> {
    fn get_cached_dist(
        &mut self,
        name: &'a uv_normalize::PackageName,
        version: uv_pep440::Version,
        expected_hash: Option<&rattler_lock::PackageHashes>,
    ) -> Option<CachedRegistryDist> {
        let index = self.get(name).find(|entry| {
            // Check version matches
            if entry.dist.filename.version != version {
                return false;
            }

            // If we have an expected hash, verify it matches
            if let Some(expected) = expected_hash {
                // Determine which hash algorithms are required and ensure each matches
                let required_algorithms = match *expected {
                    rattler_lock::PackageHashes::Md5Sha256(_, _) => {
                        vec![HashAlgorithm::Md5, HashAlgorithm::Sha256]
                    }
                    rattler_lock::PackageHashes::Md5(_) => vec![HashAlgorithm::Md5],
                    rattler_lock::PackageHashes::Sha256(_) => vec![HashAlgorithm::Sha256],
                };
                let has_matching_hash = required_algorithms.iter().all(|algorithm| {
                    entry.dist.hashes.iter().any(|hash| {
                        hash.algorithm() == *algorithm
                            && hash_matches_expected(hash, *algorithm, expected)
                    })
                });

                if !has_matching_hash {
                    return false;
                }
            }

            true
        });
        index.map(|index| index.dist.clone())
    }
}
