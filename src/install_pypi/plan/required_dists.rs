//! Utilities for creating and managing required distribution mappings for PyPI packages.
//!
//! This module provides functionality to convert PypiPackageData into Dist objects
//! and manage them in a way that satisfies lifetime requirements for the install planner.

use std::path::Path;
use std::str::FromStr;
use std::{collections::HashMap, ops::Deref};

use rattler_lock::PypiPackageData;
use uv_distribution_types::Dist;
use uv_normalize::PackageName;

use crate::install_pypi::conversions::{ConvertToUvDistError, convert_to_dist};

/// A collection of required distributions with their associated package data.
/// This struct owns the Dist objects to ensure proper lifetimes for the install planner.
pub struct RequiredDists(
    /// Map from normalized package name to (PypiPackageData, Dist)
    HashMap<PackageName, (PypiPackageData, Dist)>,
);

impl RequiredDists {
    /// Create a new RequiredDists from a slice of PypiPackageData and a lock file directory.
    ///
    /// # Arguments
    /// * `packages` - The PyPI package data to convert
    /// * `lock_file_dir` - Directory containing the lock file for resolving relative paths
    ///
    /// # Returns
    /// A RequiredDists instance or an error if conversion fails
    pub fn from_packages(
        packages: &[PypiPackageData],
        lock_file_dir: impl AsRef<Path>,
    ) -> Result<Self, ConvertToUvDistError> {
        let mut dists = HashMap::new();

        for pkg in packages {
            let uv_name = PackageName::from_str(pkg.name.as_ref()).map_err(|_| {
                ConvertToUvDistError::InvalidPackageName(pkg.name.as_ref().to_string())
            })?;
            let dist = convert_to_dist(pkg, lock_file_dir.as_ref())?;
            dists.insert(uv_name, (pkg.clone(), dist));
        }

        Ok(Self(dists))
    }

    /// Get a reference map suitable for passing to InstallPlanner::plan().
    /// Returns a map where the values are references to the owned data.
    pub fn as_ref_map(&self) -> HashMap<PackageName, (&PypiPackageData, &Dist)> {
        self.0
            .iter()
            .map(|(name, (pkg, dist))| (name.clone(), (pkg, dist)))
            .collect()
    }

    /// Get the number of required packages
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl Deref for RequiredDists {
    type Target = HashMap<PackageName, (PypiPackageData, Dist)>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
