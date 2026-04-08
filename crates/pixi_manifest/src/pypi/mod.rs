use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use pep508_rs::PackageName;

pub mod merge;
pub mod pypi_options;

/// A fully resolved PyPI exclude-newer configuration with absolute cutoffs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedPypiExcludeNewer {
    /// The default cutoff date. Packages uploaded after this date are excluded.
    pub cutoff: Option<DateTime<Utc>>,

    /// Package-specific cutoff dates that override [`Self::cutoff`].
    pub package_cutoffs: BTreeMap<PackageName, DateTime<Utc>>,
}

impl ResolvedPypiExcludeNewer {
    /// Creates a new configuration from an absolute cutoff date.
    pub fn from_datetime(cutoff: DateTime<Utc>) -> Self {
        Self {
            cutoff: Some(cutoff),
            package_cutoffs: BTreeMap::new(),
        }
    }

    /// Adds a package-specific cutoff override.
    pub fn with_package_cutoff(mut self, package: PackageName, cutoff: DateTime<Utc>) -> Self {
        self.package_cutoffs.insert(package, cutoff);
        self
    }

    /// Returns true if there is no global or package-specific cutoff configured.
    pub fn is_empty(&self) -> bool {
        self.cutoff.is_none() && self.package_cutoffs.is_empty()
    }
}
