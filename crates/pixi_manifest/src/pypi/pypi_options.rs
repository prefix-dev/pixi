use crate::consts;
use indexmap::IndexSet;
use rattler_lock::{FindLinksUrlOrPath, PypiIndexes};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::{hash::Hash, iter};
use thiserror::Error;
use url::Url;

/// Specific options for a PyPI registries
#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PypiOptions {
    /// The index URL to use as the primary pypi index
    pub index_url: Option<Url>,
    /// Any extra indexes to use, that will be searched after the primary index
    pub extra_index_urls: Option<Vec<Url>>,
    /// Flat indexes also called `--find-links` in pip
    /// These are flat listings of distributions
    pub find_links: Option<Vec<FindLinksUrlOrPath>>,
    /// Disable isolated builds
    pub no_build_isolation: Option<Vec<String>>,
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
        no_build_isolation: Option<Vec<String>>,
    ) -> Self {
        Self {
            index_url: index,
            extra_index_urls: extra_indexes,
            find_links: flat_indexes,
            no_build_isolation,
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
    /// - Extra indexes are merged and deduplicated, in the order they are provided
    /// - Flat indexes are merged and deduplicated, in the order they are provided
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

        // Merge all the no build isolation packages
        let no_build_isolation = self
            .no_build_isolation
            .as_ref()
            .map(|no_build_isolation| {
                clone_and_deduplicate(
                    no_build_isolation.iter(),
                    other.no_build_isolation.clone().unwrap_or_default().iter(),
                )
            })
            .or_else(|| other.no_build_isolation.clone());

        Ok(PypiOptions {
            index_url: index,
            extra_index_urls: extra_indexes,
            find_links: flat_indexes,
            no_build_isolation,
        })
    }
}

impl From<PypiOptions> for rattler_lock::PypiIndexes {
    fn from(value: PypiOptions) -> Self {
        let primary_index = value
            .index_url
            .unwrap_or(consts::DEFAULT_PYPI_INDEX_URL.clone());
        Self {
            indexes: iter::once(primary_index)
                .chain(value.extra_index_urls.into_iter().flatten())
                .collect(),
            find_links: value.find_links.into_iter().flatten().collect(),
        }
    }
}

impl From<&PypiOptions> for rattler_lock::PypiIndexes {
    fn from(value: &PypiOptions) -> Self {
        PypiIndexes::from(value.clone())
    }
}

#[derive(Error, Debug)]
pub enum PypiOptionsMergeError {
    #[error(
        "multiple primary pypi indexes are not supported, found both {first} and {second} across multiple pypi options"
    )]
    MultiplePrimaryIndexes { first: String, second: String },
}

#[cfg(test)]
mod tests {
    use super::PypiOptions;
    use rattler_lock::FindLinksUrlOrPath;
    use url::Url;

    #[test]
    fn test_deserialize_pypi_options() {
        let toml_str = r#"
                 index-url = "https://example.com/pypi"
                 extra-index-urls = ["https://example.com/extra"]
                 no-build-isolation = ["pkg1", "pkg2"]

                 [[find-links]]
                 path = "/path/to/flat/index"

                 [[find-links]]
                 url = "https://flat.index"
             "#;
        let deserialized_options: PypiOptions = toml_edit::de::from_str(toml_str).unwrap();
        assert_eq!(
            deserialized_options,
            PypiOptions {
                index_url: Some(Url::parse("https://example.com/pypi").unwrap()),
                extra_index_urls: Some(vec![Url::parse("https://example.com/extra").unwrap()]),
                find_links: Some(vec![
                    FindLinksUrlOrPath::Path("/path/to/flat/index".into()),
                    FindLinksUrlOrPath::Url(Url::parse("https://flat.index").unwrap())
                ]),
                no_build_isolation: Some(vec!["pkg1".to_string(), "pkg2".to_string()])
            },
        );
    }

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
            no_build_isolation: Some(vec!["foo".to_string(), "bar".to_string()]),
        };

        // Create the second set of options
        let opts2 = PypiOptions {
            index_url: None,
            extra_index_urls: Some(vec![Url::parse("https://example.com/extra2").unwrap()]),
            find_links: Some(vec![
                FindLinksUrlOrPath::Path("/path/to/flat/index2".into()),
                FindLinksUrlOrPath::Url(Url::parse("https://flat.index2").unwrap()),
            ]),
            no_build_isolation: Some(vec!["foo".to_string()]),
        };

        // Merge the two options
        // This should succeed and values should be merged
        let merged_opts = opts.union(&opts2).unwrap();
        insta::assert_yaml_snapshot!(merged_opts);
    }

    #[test]
    fn test_error_on_multiple_primary_indexes() {
        // Create the first set of options
        let opts = PypiOptions {
            index_url: Some(Url::parse("https://example.com/pypi").unwrap()),
            extra_index_urls: None,
            find_links: None,
            no_build_isolation: None,
        };

        // Create the second set of options
        let opts2 = PypiOptions {
            index_url: Some(Url::parse("https://example.com/pypi2").unwrap()),
            extra_index_urls: None,
            find_links: None,
            no_build_isolation: None,
        };

        // Merge the two options
        // This should error because there are two primary indexes
        let merged_opts = opts.union(&opts2);
        insta::assert_snapshot!(merged_opts.err().unwrap());
    }
}
