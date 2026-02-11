use std::collections::HashSet;

use indexmap::IndexMap;
use rattler_build::NormalizedKey;
use rattler_conda_types::PackageName;

use crate::matchspec::PackageDependency;

/// A package spec dependency represent dependencies for a specific target.
#[derive(Debug, Clone)]
pub struct PackageSpecDependencies<T> {
    pub build: IndexMap<PackageName, T>,
    pub host: IndexMap<PackageName, T>,
    pub run: IndexMap<PackageName, T>,
    pub run_constraints: IndexMap<PackageName, T>,
}

impl<T> Default for PackageSpecDependencies<T> {
    fn default() -> Self {
        PackageSpecDependencies {
            build: IndexMap::new(),
            host: IndexMap::new(),
            run: IndexMap::new(),
            run_constraints: IndexMap::new(),
        }
    }
}

impl PackageSpecDependencies<PackageDependency> {
    /// Return the used variants of the package spec dependencies.
    pub fn used_variants(&self) -> HashSet<NormalizedKey> {
        self.build
            .iter()
            .chain(self.host.iter())
            .chain(self.run.iter())
            .filter(|(_, spec)| spec.can_be_used_as_variant())
            .map(|(name, _)| name.clone().as_normalized().into())
            .collect()
    }

    pub fn contains(&self, name: &PackageName) -> bool {
        self.build.contains_key(name)
            || self.host.contains_key(name)
            || self.run.contains_key(name)
            || self.run_constraints.contains_key(name)
    }
}

/// Represents a platform, selector.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Selector {
    Unix,
    Linux,
    Win,
    MacOs,
    Platform(String),
}
