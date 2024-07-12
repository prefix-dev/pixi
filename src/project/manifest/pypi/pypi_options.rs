use crate::consts;
use distribution_types::{FlatIndexLocation, IndexLocations, IndexUrl};
use indexmap::IndexSet;
use pep508_rs::VerbatimUrl;
use rattler_lock::FindLinksUrlOrPath;
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
}

/// Converts to the [`distribution_types::FlatIndexLocation`]
pub fn to_flat_index_location(find_links: &FindLinksUrlOrPath) -> FlatIndexLocation {
    match find_links {
        FindLinksUrlOrPath::Path(path) => FlatIndexLocation::Path(path.clone()),
        FindLinksUrlOrPath::Url(url) => FlatIndexLocation::Url(url.clone()),
    }
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
    ) -> Self {
        Self {
            index_url: index,
            extra_index_urls: extra_indexes,
            find_links: flat_indexes,
        }
    }

    /// Converts to the [`distribution_types::IndexLocations`]
    pub fn to_index_locations(&self) -> IndexLocations {
        // Convert the index to a `IndexUrl`
        let index = self
            .index_url
            .clone()
            .map(VerbatimUrl::from_url)
            .map(IndexUrl::from);

        // Convert to list of extra indexes
        let extra_indexes = self
            .extra_index_urls
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
            .find_links
            .clone()
            .map(|indexes| {
                indexes
                    .into_iter()
                    .map(|index| to_flat_index_location(&index))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // We keep the `no_index` to false for now, because I've not seen a use case for it yet
        // we could change this later if needed
        IndexLocations::new(index, extra_indexes, flat_indexes, false)
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

        Ok(PypiOptions {
            index_url: index,
            extra_index_urls: extra_indexes,
            find_links: flat_indexes,
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

#[derive(Error, Debug)]
pub enum PypiOptionsMergeError {
    #[error(
        "multiple primary pypi indexes are not supported, found both {first} and {second} across multiple pypi options"
    )]
    MultiplePrimaryIndexes { first: String, second: String },
}

#[cfg(test)]
mod tests {
    use crate::project::manifest::pypi::pypi_options::PypiOptions;
    use rattler_lock::FindLinksUrlOrPath;
    use url::Url;

    #[test]
    fn test_deserialize_pypi_options() {
        let toml_str = r#"
                 index-url = "https://example.com/pypi"
                 extra-index-urls = ["https://example.com/extra"]

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
                ])
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
        };

        // Create the second set of options
        let opts2 = PypiOptions {
            index_url: None,
            extra_index_urls: Some(vec![Url::parse("https://example.com/extra2").unwrap()]),
            find_links: Some(vec![
                FindLinksUrlOrPath::Path("/path/to/flat/index2".into()),
                FindLinksUrlOrPath::Url(Url::parse("https://flat.index2").unwrap()),
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
            index_url: Some(Url::parse("https://example.com/pypi").unwrap()),
            extra_index_urls: None,
            find_links: None,
        };

        // Create the second set of options
        let opts2 = PypiOptions {
            index_url: Some(Url::parse("https://example.com/pypi2").unwrap()),
            extra_index_urls: None,
            find_links: None,
        };

        // Merge the two options
        let merged_opts = opts.union(&opts2);
        insta::assert_snapshot!(merged_opts.err().unwrap());
    }
}
