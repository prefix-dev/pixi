use std::{collections::HashSet, hash::Hash, path::PathBuf};

use indexmap::IndexSet;
use pep508_rs::PackageName;
use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};
use thiserror::Error;
use url::Url;

// taken from: https://docs.astral.sh/uv/reference/settings/#index-strategy
/// The strategy to use when resolving against multiple index URLs.
/// By default, uv will stop at the first index on which a given package is
/// available, and limit resolutions to those present on that first index
/// (first-match). This prevents "dependency confusion" attacks, whereby an
/// attack can upload a malicious package under the same name to a secondary.
#[derive(
    Default,
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    strum::Display,
    strum::EnumString,
    strum::VariantNames,
)]
#[strum(serialize_all = "kebab-case")]
#[serde(rename_all = "kebab-case")]
pub enum IndexStrategy {
    #[default]
    /// Only use results from the first index that returns a match for a given
    /// package name
    FirstIndex,
    /// Search for every package name across all indexes, exhausting the
    /// versions from the first index before moving on to the next
    UnsafeFirstMatch,
    /// Search for every package name across all indexes, preferring the "best"
    /// version found. If a package version is in multiple indexes, only look at
    /// the entry for the first index
    UnsafeBestMatch,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum FindLinksUrlOrPath {
    /// Can be a path to a directory or a file containing the flat index
    Path(PathBuf),

    /// Can be a URL to a flat index
    Url(Url),
}

/// Don't build sdist for all or certain packages
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, strum::Display)]
#[strum(serialize_all = "kebab-case")]
#[serde(rename_all = "kebab-case")]
pub enum NoBuild {
    /// Build any sdist we come across
    #[default]
    None,
    /// Don't build any sdist
    All,
    /// Don't build sdist for specific packages
    // Todo: would be nice to check if these are actually used at some point
    Packages(HashSet<pep508_rs::PackageName>),
}

impl NoBuild {
    /// Merges two `NoBuild` together, according to the following rules
    /// - If either is `All`, the result is `All`
    /// - If either is `None`, the result is the other
    /// - If both are `Packages`, the result is the union of the two
    pub fn union(&self, other: &NoBuild) -> NoBuild {
        match (self, other) {
            (NoBuild::All, _) | (_, NoBuild::All) => NoBuild::All,
            (NoBuild::None, _) => other.clone(),
            (_, NoBuild::None) => self.clone(),
            (NoBuild::Packages(packages), NoBuild::Packages(other_packages)) => {
                let mut packages = packages.clone();
                packages.extend(other_packages.iter().cloned());
                NoBuild::Packages(packages)
            }
        }
    }
}

/// Don't install pre-built wheels for all or certain packages
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, strum::Display)]
#[strum(serialize_all = "kebab-case")]
#[serde(rename_all = "kebab-case")]
pub enum NoBinary {
    /// Use pre-built wheels for all package
    #[default]
    None,
    /// Build all package from source
    All,
    /// Build specific packages from source
    Packages(HashSet<pep508_rs::PackageName>),
}

impl NoBinary {
    /// Merges two `NoBinary` together, according to the following rules
    /// - If either is `All`, the result is `All`
    /// - If either is `None`, the result is the other
    /// - If both are `Packages`, the result is the union of the two
    pub fn union(&self, other: &NoBinary) -> NoBinary {
        match (self, other) {
            (NoBinary::All, _) | (_, NoBinary::All) => NoBinary::All,
            (NoBinary::None, _) => other.clone(),
            (_, NoBinary::None) => self.clone(),
            (NoBinary::Packages(packages), NoBinary::Packages(other_packages)) => {
                let mut packages = packages.clone();
                packages.extend(other_packages.iter().cloned());
                NoBinary::Packages(packages)
            }
        }
    }
}

/// Specific options for a PyPI registries
#[derive(Debug, Clone, PartialEq, Serialize, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub struct PypiOptions {
    /// The index URL to use as the primary pypi index
    pub index_url: Option<Url>,
    /// Any extra indexes to use, that will be searched after the primary index
    pub extra_index_urls: Option<Vec<Url>>,
    /// Flat indexes also called `--find-links` in pip
    /// These are flat listings of distributions
    pub find_links: Option<Vec<FindLinksUrlOrPath>>,
    /// Disable isolated builds
    pub no_build_isolation: NoBuildIsolation,
    /// The strategy to use when resolving against multiple index URLs.
    pub index_strategy: Option<IndexStrategy>,
    /// Don't build sdist for all or certain packages
    pub no_build: Option<NoBuild>,
    /// Don't use pre-built wheels all or certain packages
    pub no_binary: Option<NoBinary>,
}

/// Clones and deduplicates two iterators of values
fn clone_and_deduplicate<'a, I: Iterator<Item = &'a T>, T: Clone + Eq + Hash + 'a>(
    values: I,
    other: I,
) -> Vec<T> {
    values
        .cloned()
        .chain(other.cloned())
        .collect::<IndexSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
}

impl PypiOptions {
    pub fn new(
        index: Option<Url>,
        extra_indexes: Option<Vec<Url>>,
        flat_indexes: Option<Vec<FindLinksUrlOrPath>>,
        no_build_isolation: NoBuildIsolation,
        index_strategy: Option<IndexStrategy>,
        no_build: Option<NoBuild>,
        no_binary: Option<NoBinary>,
    ) -> Self {
        Self {
            index_url: index,
            extra_index_urls: extra_indexes,
            find_links: flat_indexes,
            no_build_isolation,
            index_strategy,
            no_build,
            no_binary,
        }
    }

    /// Return an iterator over all [`Url`] entries.
    /// In order of:
    /// - `find_links`
    /// - `extra_index_urls`
    /// - `index_url`
    pub fn urls(&self) -> impl Iterator<Item = &'_ Url> + '_ {
        let find_links = self
            .find_links
            .iter()
            .flatten()
            .filter_map(|index| match index {
                FindLinksUrlOrPath::Path(_) => None,
                FindLinksUrlOrPath::Url(url) => Some(url),
            });

        let extra_indexes = self.extra_index_urls.iter().flatten();
        find_links
            .chain(extra_indexes)
            .chain(std::iter::once(self.index_url.as_ref()).flatten())
    }

    /// Merges two `PypiOptions` together, according to the following rules
    /// - There can only be one primary index
    /// - Extra indexes are merged and deduplicated, in the order they are
    ///   provided
    /// - Flat indexes are merged and deduplicated, in the order they are
    ///   provided
    pub fn union(&self, other: &PypiOptions) -> Result<PypiOptions, PypiOptionsMergeError> {
        let index = if let Some(other_index) = other.index_url.clone() {
            // Allow only one index
            if let Some(own_index) = self.index_url.clone() {
                return Err(PypiOptionsMergeError::MultiplePrimaryIndexes {
                    first: own_index.to_string(),
                    second: other_index.to_string(),
                });
            } else {
                // Use the other index, because we don't have one
                Some(other_index)
            }
        } else {
            // Use our index, because the other doesn't have one
            self.index_url.clone()
        };

        // Allow only one index strategy
        let index_strategy = if let Some(other_index_strategy) = other.index_strategy.clone() {
            if let Some(own_index_strategy) = &self.index_strategy {
                return Err(PypiOptionsMergeError::MultipleIndexStrategies {
                    first: own_index_strategy.to_string(),
                    second: other_index_strategy.to_string(),
                });
            } else {
                Some(other_index_strategy)
            }
        } else {
            self.index_strategy.clone()
        };

        // Chain together and deduplicate the extra indexes
        let extra_indexes = self
            .extra_index_urls
            .as_ref()
            // Map for value
            .map(|extra_indexes| {
                clone_and_deduplicate(
                    extra_indexes.iter(),
                    other.extra_index_urls.clone().unwrap_or_default().iter(),
                )
            })
            .or_else(|| other.extra_index_urls.clone());

        // Chain together and deduplicate the flat indexes
        let flat_indexes = self
            .find_links
            .as_ref()
            .map(|flat_indexes| {
                clone_and_deduplicate(
                    flat_indexes.iter(),
                    other.find_links.clone().unwrap_or_default().iter(),
                )
            })
            .or_else(|| other.find_links.clone());

        // Merge all the no build isolation packages. We take the union.
        let no_build_isolation = match (&self.no_build_isolation, &other.no_build_isolation) {
            (NoBuildIsolation::All, _) | (_, NoBuildIsolation::All) => NoBuildIsolation::All,
            (NoBuildIsolation::Packages(a), NoBuildIsolation::Packages(b)) => {
                let mut packages = a.clone();
                packages.extend(b.iter().cloned());
                NoBuildIsolation::Packages(packages)
            }
        };

        // Set the no-build option
        let no_build = match (self.no_build.as_ref(), other.no_build.as_ref()) {
            (Some(a), Some(b)) => Some(a.union(b)),
            (Some(a), None) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        };

        // Set the no-binary option
        let no_binary = match (self.no_binary.as_ref(), other.no_binary.as_ref()) {
            (Some(a), Some(b)) => Some(a.union(b)),
            (Some(a), None) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        };

        Ok(PypiOptions {
            index_url: index,
            extra_index_urls: extra_indexes,
            find_links: flat_indexes,
            no_build_isolation,
            index_strategy,
            no_build,
            no_binary,
        })
    }
}

#[cfg(feature = "rattler_lock")]
impl From<PypiOptions> for rattler_lock::PypiIndexes {
    fn from(value: PypiOptions) -> Self {
        let primary_index = value
            .index_url
            .unwrap_or(pixi_consts::consts::DEFAULT_PYPI_INDEX_URL.clone());
        Self {
            indexes: std::iter::once(primary_index)
                .chain(value.extra_index_urls.into_iter().flatten())
                .collect(),
            find_links: value
                .find_links
                .into_iter()
                .flatten()
                .map(Into::into)
                .collect(),
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl From<FindLinksUrlOrPath> for rattler_lock::FindLinksUrlOrPath {
    fn from(value: FindLinksUrlOrPath) -> Self {
        match value {
            FindLinksUrlOrPath::Path(path) => rattler_lock::FindLinksUrlOrPath::Path(path),
            FindLinksUrlOrPath::Url(url) => rattler_lock::FindLinksUrlOrPath::Url(url),
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl From<rattler_lock::FindLinksUrlOrPath> for FindLinksUrlOrPath {
    fn from(value: rattler_lock::FindLinksUrlOrPath) -> Self {
        match value {
            rattler_lock::FindLinksUrlOrPath::Path(path) => FindLinksUrlOrPath::Path(path),
            rattler_lock::FindLinksUrlOrPath::Url(url) => FindLinksUrlOrPath::Url(url),
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl From<&PypiOptions> for rattler_lock::PypiIndexes {
    fn from(value: &PypiOptions) -> Self {
        rattler_lock::PypiIndexes::from(value.clone())
    }
}

#[derive(Error, Debug)]
pub enum PypiOptionsMergeError {
    #[error(
        "multiple primary pypi indexes are not supported, found both {first} and {second} across multiple pypi options"
    )]
    MultiplePrimaryIndexes { first: String, second: String },
    #[error(
        "multiple index strategies are not supported, found both {first} and {second} across multiple pypi options"
    )]
    MultipleIndexStrategies { first: String, second: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoBuildIsolation {
    /// Don't use build isolation for any package.
    All,

    /// Do not use build isolation for a specific set of package.
    Packages(IndexSet<pep508_rs::PackageName>),
}

impl Serialize for NoBuildIsolation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            NoBuildIsolation::All => serializer.serialize_bool(true),
            NoBuildIsolation::Packages(packages) => {
                let mut seq = serializer.serialize_seq(Some(packages.len()))?;
                for package in packages {
                    seq.serialize_element(package)?;
                }
                seq.end()
            }
        }
    }
}

impl Default for NoBuildIsolation {
    fn default() -> Self {
        NoBuildIsolation::none()
    }
}

impl NoBuildIsolation {
    /// Create a new `NoBuildIsolation` where all packages are build isolated.
    pub fn none() -> Self {
        NoBuildIsolation::Packages(IndexSet::new())
    }

    /// Returns the union of two `NoBuildIsolation` values.
    pub fn union(&self, other: &NoBuildIsolation) -> NoBuildIsolation {
        match (self, other) {
            (NoBuildIsolation::All, _) | (_, NoBuildIsolation::All) => NoBuildIsolation::All,
            (NoBuildIsolation::Packages(a), NoBuildIsolation::Packages(b)) => {
                let mut packages = a.clone();
                packages.extend(b.iter().cloned());
                NoBuildIsolation::Packages(packages)
            }
        }
    }

    /// Returns true if the given package is in the set of packages that should
    /// *not* use build isolation.
    pub fn contains(&self, package: &pep508_rs::PackageName) -> bool {
        match self {
            NoBuildIsolation::All => true,
            NoBuildIsolation::Packages(packages) => packages.contains(package),
        }
    }
}

impl FromIterator<pep508_rs::PackageName> for NoBuildIsolation {
    fn from_iter<T: IntoIterator<Item = PackageName>>(iter: T) -> Self {
        NoBuildIsolation::Packages(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{PypiOptions, *};
    use crate::pypi::pypi_options::IndexStrategy;

    #[test]
    fn test_merge_pypi_options() {
        // Create the first set of options
        let opts = PypiOptions {
            index_url: Some(Url::parse("https://example.com/pypi").unwrap()),
            extra_index_urls: Some(vec![Url::parse("https://example.com/extra").unwrap()]),
            find_links: Some(vec![
                FindLinksUrlOrPath::Path("/path/to/flat/index".into()),
                FindLinksUrlOrPath::Url(Url::parse("https://flat.index").unwrap()),
            ]),
            no_build_isolation: NoBuildIsolation::from_iter([
                "foo".parse().unwrap(),
                "bar".parse().unwrap(),
            ]),
            index_strategy: None,
            no_build: None,
            no_binary: Default::default(),
        };

        // Create the second set of options
        let opts2 = PypiOptions {
            index_url: None,
            extra_index_urls: Some(vec![Url::parse("https://example.com/extra2").unwrap()]),
            find_links: Some(vec![
                FindLinksUrlOrPath::Path("/path/to/flat/index2".into()),
                FindLinksUrlOrPath::Url(Url::parse("https://flat.index2").unwrap()),
            ]),
            no_build_isolation: NoBuildIsolation::from_iter(["foo".parse().unwrap()]),
            index_strategy: None,
            no_build: Some(NoBuild::All),
            no_binary: Default::default(),
        };

        // Merge the two options
        // This should succeed and values should be merged
        let merged_opts = opts.union(&opts2).unwrap();
        insta::assert_yaml_snapshot!(merged_opts);
    }

    #[test]
    fn test_no_build_union() {
        let pkg1 = pep508_rs::PackageName::new("pkg1".to_string()).unwrap();
        let pkg2 = pep508_rs::PackageName::new("pkg1".to_string()).unwrap();
        let pkg3 = pep508_rs::PackageName::new("pkg1".to_string()).unwrap();

        // Case 1: One is `All`, result should be `All`
        assert_eq!(NoBuild::All.union(&NoBuild::None), NoBuild::All);
        assert_eq!(NoBuild::None.union(&NoBuild::All), NoBuild::All);
        assert_eq!(
            NoBuild::All.union(&NoBuild::Packages(HashSet::from_iter([pkg1.clone()]))),
            NoBuild::All
        );

        // Case 2: One is `None`, result should be the other
        assert_eq!(NoBuild::None.union(&NoBuild::None), NoBuild::None);
        assert_eq!(
            NoBuild::None.union(&NoBuild::Packages(HashSet::from_iter([pkg1.clone()]))),
            NoBuild::Packages(HashSet::from_iter([pkg1.clone()]))
        );
        assert_eq!(
            NoBuild::Packages(HashSet::from_iter([pkg1.clone()])).union(&NoBuild::None),
            NoBuild::Packages(HashSet::from_iter([pkg1.clone()]))
        );

        // Case 3: Both are `Packages`, result should be the union of the two
        assert_eq!(
            NoBuild::Packages(HashSet::from_iter([pkg1.clone(), pkg2.clone()])).union(
                &NoBuild::Packages(HashSet::from_iter([pkg2.clone(), pkg3.clone()]))
            ),
            NoBuild::Packages(HashSet::from_iter([
                pkg1.clone(),
                pkg2.clone(),
                pkg3.clone()
            ]))
        );
    }

    #[test]
    fn test_no_binary_union() {
        let pkg1 = pep508_rs::PackageName::new("pkg1".to_string()).unwrap();
        let pkg2 = pep508_rs::PackageName::new("pkg1".to_string()).unwrap();
        let pkg3 = pep508_rs::PackageName::new("pkg1".to_string()).unwrap();

        // Case 1: One is `All`, result should be `All`
        assert_eq!(NoBinary::All.union(&NoBinary::None), NoBinary::All);
        assert_eq!(NoBinary::None.union(&NoBinary::All), NoBinary::All);
        assert_eq!(
            NoBinary::All.union(&NoBinary::Packages(HashSet::from_iter([pkg1.clone()]))),
            NoBinary::All
        );

        // Case 2: One is `None`, result should be the other
        assert_eq!(NoBinary::None.union(&NoBinary::None), NoBinary::None);
        assert_eq!(
            NoBinary::None.union(&NoBinary::Packages(HashSet::from_iter([pkg1.clone()]))),
            NoBinary::Packages(HashSet::from_iter([pkg1.clone()]))
        );
        assert_eq!(
            NoBinary::Packages(HashSet::from_iter([pkg1.clone()])).union(&NoBinary::None),
            NoBinary::Packages(HashSet::from_iter([pkg1.clone()]))
        );

        // Case 3: Both are `Packages`, result should be the union of the two
        assert_eq!(
            NoBinary::Packages(HashSet::from_iter([pkg1.clone(), pkg2.clone()])).union(
                &NoBinary::Packages(HashSet::from_iter([pkg2.clone(), pkg3.clone()]))
            ),
            NoBinary::Packages(HashSet::from_iter([
                pkg1.clone(),
                pkg2.clone(),
                pkg3.clone()
            ]))
        );
    }

    #[test]
    fn test_error_on_multiple_primary_indexes() {
        // Create the first set of options
        let opts = PypiOptions {
            index_url: Some(Url::parse("https://example.com/pypi").unwrap()),
            extra_index_urls: None,
            find_links: None,
            no_build_isolation: NoBuildIsolation::default(),
            index_strategy: None,
            no_build: Default::default(),
            no_binary: Default::default(),
        };

        // Create the second set of options
        let opts2 = PypiOptions {
            index_url: Some(Url::parse("https://example.com/pypi2").unwrap()),
            extra_index_urls: None,
            find_links: None,
            no_build_isolation: NoBuildIsolation::default(),
            index_strategy: None,
            no_build: Default::default(),
            no_binary: Default::default(),
        };

        // Merge the two options
        // This should error because there are two primary indexes
        let merged_opts = opts.union(&opts2);
        insta::assert_snapshot!(merged_opts.err().unwrap());
    }

    #[test]
    fn test_error_on_multiple_index_strategies() {
        // Create the first set of options
        let opts = PypiOptions {
            index_url: None,
            extra_index_urls: None,
            find_links: None,
            no_build_isolation: NoBuildIsolation::default(),
            index_strategy: Some(IndexStrategy::FirstIndex),
            no_build: Default::default(),
            no_binary: Default::default(),
        };

        // Create the second set of options
        let opts2 = PypiOptions {
            index_url: None,
            extra_index_urls: None,
            find_links: None,
            no_build_isolation: NoBuildIsolation::default(),
            index_strategy: Some(IndexStrategy::UnsafeBestMatch),
            no_build: Default::default(),
            no_binary: Default::default(),
        };

        // Merge the two options
        // This should error because there are two index strategies
        let merged_opts = opts.union(&opts2);
        insta::assert_snapshot!(merged_opts.err().unwrap());
    }
}
