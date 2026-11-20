use std::collections::{BTreeMap, BTreeSet};

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
    ///
    /// Deprecated in favor of [`Self::exempt_packages`]. Only the deprecated
    /// top-level `[pypi-exclude-newer]` manifest table still populates this.
    pub package_cutoffs: BTreeMap<PackageName, DateTime<Utc>>,

    /// Packages that are never excluded, regardless of their upload time.
    pub exempt_packages: BTreeSet<PackageName>,
}

impl ResolvedPypiExcludeNewer {
    /// Creates a new configuration from an absolute cutoff date.
    pub fn from_datetime(cutoff: DateTime<Utc>) -> Self {
        Self {
            cutoff: Some(cutoff),
            package_cutoffs: BTreeMap::new(),
            exempt_packages: BTreeSet::new(),
        }
    }

    /// Adds a package-specific cutoff override.
    ///
    /// Deprecated in favor of [`Self::with_exempt_package`].
    #[deprecated(note = "use `with_exempt_package` to exempt a package from the cutoff instead")]
    pub fn with_package_cutoff(mut self, package: PackageName, cutoff: DateTime<Utc>) -> Self {
        self.package_cutoffs.insert(package, cutoff);
        self
    }

    /// Exempts every release of `package` from the cutoff.
    pub fn with_exempt_package(mut self, package: PackageName) -> Self {
        self.exempt_packages.insert(package);
        self
    }

    /// Returns true if there is no global or package-specific cutoff configured.
    pub fn is_empty(&self) -> bool {
        self.cutoff.is_none() && self.package_cutoffs.is_empty() && self.exempt_packages.is_empty()
    }
}
