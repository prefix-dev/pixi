use std::{
    hash::Hash,
    path::{Path, PathBuf},
};

use distribution_types::{FlatIndexLocation, IndexLocations, IndexUrl};
use indexmap::IndexSet;
use pep508_rs::VerbatimUrl;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use thiserror::Error;
use url::Url;

/// Specific options for a PyPI registries
#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PypiOptions {
    /// The index URL to use as the primary pypi index
    pub index: Option<Url>,
    /// Any extra indexes to use, that will be searched after the primary index
    pub extra_indexes: Option<Vec<Url>>,
    /// Flat indexes also called `--find-links` in pip
    /// These are flat listings of distributions
    pub flat_indexes: Option<Vec<FlatIndexUrlOrPath>>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum FlatIndexUrlOrPath {
    /// Can be a path to a directory or a file
    /// containinin the flat index
    Path(PathBuf),
    /// Can be a URL to a flat index
    Url(Url),
}

impl FlatIndexUrlOrPath {
    /// Returns the URL if it is a URL
    pub fn as_url(&self) -> Option<&Url> {
        match self {
            Self::Path(_) => None,
            Self::Url(url) => Some(url),
        }
    }

    /// Returns the path if it is a path
    pub fn as_path(&self) -> Option<&Path> {
        match self {
            Self::Path(path) => Some(path),
            Self::Url(_) => None,
        }
    }

    /// Converts to the [`distribution_types::FlatIndexLocation`]
    pub fn to_flat_index_location(&self) -> FlatIndexLocation {
        match self {
            Self::Path(path) => FlatIndexLocation::Path(path.clone()),
            Self::Url(url) => FlatIndexLocation::Url(url.clone()),
        }
    }
}

impl PypiOptions {
    pub fn new(
        index: Option<Url>,
        extra_indexes: Option<Vec<Url>>,
        flat_indexes: Option<Vec<FlatIndexUrlOrPath>>,
    ) -> Self {
        Self {
            index,
            extra_indexes,
            flat_indexes,
        }
    }

    /// Converts to the [`distribution_types::IndexLocations`]
    pub fn to_index_locations(&self) -> IndexLocations {
        // Convert the index to a `IndexUrl`
        let index = self
            .index
            .clone()
            .map(VerbatimUrl::from_url)
            .map(IndexUrl::from);

        // Convert to list of extra indexes
        let extra_indexes = self
            .extra_indexes
            .clone()
            .map(|urls| {
                urls.into_iter()
                    .map(VerbatimUrl::from_url)
                    .map(IndexUrl::from)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // Convert to list of flat indexes
        let flat_indexes = self
            .flat_indexes
            .clone()
            .map(|indexes| {
                indexes
                    .into_iter()
                    .map(|index| index.to_flat_index_location())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // We keep the `no_index` to false for now, because I've not seen a use case for it yet
        // we could change this later if needed
        IndexLocations::new(index, extra_indexes, flat_indexes, false)
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

    /// Merges two `PypiOptions` together, according to the following rules
    /// - There can only be one primary index
    /// - Extra indexes are merged and deduplicated, in the order they are provided
    /// - Flat indexes are merged and deduplicated, in the order they are provided
    pub fn union(&self, other: &PypiOptions) -> Result<PypiOptions, PypiOptionsMergeError> {
        // Allow only one index
        let index = if other.index.is_some() {
            if self.index.is_some() {
                return Err(PypiOptionsMergeError::MultiplePrimaryIndexes);
            } else {
                other.index.clone()
            }
        } else {
            self.index.clone()
        };

        // Chain together and deduplicate the extra indexes
        let extra_indexes = self
            .extra_indexes
            .as_ref()
            // Map for value
            .map(|extra_indexes| {
                Self::clone_and_deduplicate(
                    extra_indexes.iter(),
                    other.extra_indexes.clone().unwrap_or_default().iter(),
                )
            })
            .or_else(|| other.extra_indexes.clone());

        // Chain together and deduplicate the flat indexes
        let flat_indexes = self
            .flat_indexes
            .as_ref()
            .map(|flat_indexes| {
                Self::clone_and_deduplicate(
                    flat_indexes.iter(),
                    other.flat_indexes.clone().unwrap_or_default().iter(),
                )
            })
            .or_else(|| other.flat_indexes.clone());

        Ok(PypiOptions {
            index,
            extra_indexes,
            flat_indexes,
        })
    }
}

#[derive(Error, Debug)]
pub enum PypiOptionsMergeError {
    #[error("Multiple primary pypi indexes are not supported")]
    MultiplePrimaryIndexes,
}

#[cfg(test)]
mod tests {
    use crate::project::manifest::pypi_options::{FlatIndexUrlOrPath, PypiOptions};
    use url::Url;

    #[test]
    fn test_deserialize_pypi_options() {
        let toml_str = r#"
                 index = "https://example.com/pypi"
                 extra-indexes = ["https://example.com/extra"]

                 [[flat-indexes]]
                 path = "/path/to/flat/index"

                 [[flat-indexes]]
                 url = "https://flat.index"
             "#;
        let deserialized_options: PypiOptions = toml::from_str(toml_str).unwrap();
        assert_eq!(
            deserialized_options,
            PypiOptions {
                index: Some(Url::parse("https://example.com/pypi").unwrap()),
                extra_indexes: Some(vec![Url::parse("https://example.com/extra").unwrap()]),
                flat_indexes: Some(vec![
                    FlatIndexUrlOrPath::Path("/path/to/flat/index".into()),
                    FlatIndexUrlOrPath::Url(Url::parse("https://flat.index").unwrap())
                ])
            },
        );
    }

    #[test]
    fn test_merge_pypi_options() {
        // Create the first set of options
        let opts = PypiOptions {
            index: Some(Url::parse("https://example.com/pypi").unwrap()),
            extra_indexes: Some(vec![Url::parse("https://example.com/extra").unwrap()]),
            flat_indexes: Some(vec![
                FlatIndexUrlOrPath::Path("/path/to/flat/index".into()),
                FlatIndexUrlOrPath::Url(Url::parse("https://flat.index").unwrap()),
            ]),
        };

        // Create the second set of options
        let opts2 = PypiOptions {
            index: None,
            extra_indexes: Some(vec![Url::parse("https://example.com/extra2").unwrap()]),
            flat_indexes: Some(vec![
                FlatIndexUrlOrPath::Path("/path/to/flat/index2".into()),
                FlatIndexUrlOrPath::Url(Url::parse("https://flat.index2").unwrap()),
            ]),
        };

        // Merge the two options
        let merged_opts = opts.union(&opts2).unwrap();
        insta::assert_yaml_snapshot!(merged_opts);
    }

    #[test]
    fn test_error_on_multiple_primary_indexes() {
        // Create the first set of options
        let opts = PypiOptions {
            index: Some(Url::parse("https://example.com/pypi").unwrap()),
            extra_indexes: None,
            flat_indexes: None,
        };

        // Create the second set of options
        let opts2 = PypiOptions {
            index: Some(Url::parse("https://example.com/pypi2").unwrap()),
            extra_indexes: None,
            flat_indexes: None,
        };

        // Merge the two options
        let merged_opts = opts.union(&opts2);
        assert!(merged_opts.is_err());
    }
}
