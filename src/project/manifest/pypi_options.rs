use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};
use serde_with::serde_as;
use url::Url;

/// Specific options for a PyPI registries
#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Eq, Deserialize)]
pub struct PypiOptions {
    /// The index URL to use as the primary pypi index
    pub index: Option<Url>,
    /// Any extra indexes to use, that will be searched after the primary index
    pub extra_indexes: Option<Vec<Url>>,
    /// Flat indexes also called `--find-links` in pip
    /// These are flat listings of disributions
    pub flat_indexes: Option<Vec<FlatIndexUrlOrPath>>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum FlatIndexUrlOrPath {
    /// Can be a path to a directory or a file
    /// containinin the flat index
    Path(PathBuf),
    /// Can be a URL to a flat index
    Url(Url),
}

#[cfg(test)]
mod tests {
    use crate::project::manifest::pypi_options::{FlatIndexUrlOrPath, PypiOptions};
    use url::Url;

    #[test]
    fn test_deserialize_pypi_options() {
        let toml_str = r#"
                 index = "https://example.com/pypi"
                 extra_indexes = ["https://example.com/extra"]

                 [[flat_indexes]]
                 path = "/path/to/flat/index"

                 [[flat_indexes]]
                 url = "https://flat.index"
             "#;
        let deserialized_options: PypiOptions = toml::from_str(toml_str).unwrap();
        assert_eq!(
            deserialized_options,
            PypiOptions {
                index: Some(Url::parse("https://pypi.org/simple").unwrap()),
                extra_indexes: Some(vec![Url::parse("https://mypi.org/simple").unwrap()]),
                flat_indexes: Some(vec![
                    FlatIndexUrlOrPath::Path("path/to/flat/index".into()),
                    FlatIndexUrlOrPath::Url(Url::parse("https://flat.index").unwrap())
                ])
            },
        );
    }
}
