use pep440_rs::VersionSpecifiers;
use pep508_rs::VerbatimUrl;
use serde::{de, de::Error, Deserialize, Deserializer, Serialize};
use std::path::PathBuf;
use std::{fmt, fmt::Formatter, str::FromStr};
use thiserror::Error;
use toml_edit::Item;
use url::Url;

use uv_normalize::{ExtraName, InvalidNameError, PackageName};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct PyPiPackageName {
    source: String,
    normalized: PackageName,
}

impl<'de> Deserialize<'de> for PyPiPackageName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .string(|str| PyPiPackageName::from_str(str).map_err(Error::custom))
            .expecting("a string")
            .deserialize(deserializer)
    }
}

impl PyPiPackageName {
    pub fn from_str(name: &str) -> Result<Self, InvalidNameError> {
        Ok(Self {
            source: name.to_string(),
            normalized: uv_normalize::PackageName::from_str(name)?,
        })
    }
    pub fn from_normalized(normalized: PackageName) -> Self {
        Self {
            source: normalized.as_ref().to_string(),
            normalized,
        }
    }

    pub fn as_normalized(&self) -> &PackageName {
        &self.normalized
    }

    pub fn as_source(&self) -> &str {
        &self.source
    }
}

impl FromStr for PyPiPackageName {
    type Err = InvalidNameError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::from_str(name)
    }
}

#[derive(Debug, Default, Clone, Eq, PartialEq, Serialize)]
pub struct PyPiRequirement {
    #[serde(flatten)]
    pub(crate) requirement: PyPiRequirementType,
    pub(crate) extras: Option<Vec<ExtraName>>,
}

#[derive(Default, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct VersionOrStar {
    pub(crate) version: Option<VersionSpecifiers>,
    pub(crate) index: Option<String>,
}

impl FromStr for VersionOrStar {
    type Err = ParsePyPiRequirementError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        if s.is_empty() {
            return Err(ParsePyPiRequirementError::EmptyStringNotAllowed);
        }
        if s == "*" {
            // Accept a star as an any requirement, which is represented by the none.
            Ok(Self::default())
        } else if s.starts_with(|c: char| c.is_ascii_digit()) {
            Err(ParsePyPiRequirementError::MissingOperator(s.to_string()))
        } else {
            // From string can only parse the version specifier.
            Ok(Self {
                version: Some(
                    pep440_rs::VersionSpecifiers::from_str(s)
                        .map_err(ParsePyPiRequirementError::Pep440Error)?,
                ),
                index: None,
            })
        }
    }
}

impl<'de> Deserialize<'de> for VersionOrStar {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .string(|str| VersionOrStar::from_str(str).map_err(Error::custom))
            .map(|map| {
                #[derive(Deserialize)]
                pub struct RawVersionOrStar {
                    version: Option<String>,
                    index: Option<String>,
                }
                let raw_version_or_star =
                    RawVersionOrStar::deserialize(de::value::MapAccessDeserializer::new(map))?;
                let mut version = None;
                if let Some(raw_version) = raw_version_or_star.version {
                    if raw_version != "*" {
                        version = Some(
                            VersionSpecifiers::from_str(raw_version.as_str())
                                .map_err(Error::custom)?,
                        );
                    }
                };
                Ok(VersionOrStar {
                    version,
                    index: raw_version_or_star.index,
                })
            })
            .expecting("either a map or a string")
            .deserialize(deserializer)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(untagged, rename_all = "snake_case")]
pub enum PyPiRequirementType {
    Git {
        git: Url,
        branch: Option<String>,
        tag: Option<String>,
        rev: Option<String>,
        subdirectory: Option<String>,
    },
    Path {
        path: PathBuf,
        editable: Option<bool>,
    },
    Url {
        url: Url,
    },
    // Always try last to avoid serializing as version when it is not.
    Version(VersionOrStar),
}

impl Default for PyPiRequirementType {
    fn default() -> Self {
        PyPiRequirementType::Version(VersionOrStar::default())
    }
}

/// The type of parse error that occurred when parsing match spec.
#[derive(Debug, Clone, Error)]
pub enum ParsePyPiRequirementError {
    #[error("invalid pep440 version specifier")]
    Pep440Error(#[from] pep440_rs::VersionSpecifiersParseError),

    #[error("empty string is not allowed, did you mean '*'?")]
    EmptyStringNotAllowed,

    #[error("missing operator in version specifier, did you mean '~={0}'?")]
    MissingOperator(String),
}

impl fmt::Display for PyPiRequirement {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let item: Item = self.clone().into();
        write!(f, "{item}")
    }
}

impl From<PyPiRequirement> for Item {
    /// PyPiRequirement to a toml_edit item, to put in the manifest file.
    fn from(val: PyPiRequirement) -> Item {
        let mut req_item = match val.requirement {
            PyPiRequirementType::Version(VersionOrStar { version, index }) => {
                if let (Some(version), Some(index)) = (&version, &index) {
                    let mut table = toml_edit::Table::new().into_inline_table();
                    table.insert(
                        "version",
                        toml_edit::Value::String(toml_edit::Formatted::new(version.to_string())),
                    );
                    table.insert(
                        "index",
                        toml_edit::Value::String(toml_edit::Formatted::new(index.to_string())),
                    );
                    Item::Value(toml_edit::Value::InlineTable(table))
                } else if let Some(version) = version {
                    // When there are extras, prepare inline table.
                    if val.extras.is_some() {
                        let mut table = toml_edit::Table::new().into_inline_table();
                        table.insert(
                            "version",
                            toml_edit::Value::String(toml_edit::Formatted::new(
                                version.to_string(),
                            )),
                        );
                        Item::Value(toml_edit::Value::InlineTable(table))
                    } else {
                        // When there are no extras, just use the string representation.
                        Item::Value(toml_edit::Value::String(toml_edit::Formatted::new(
                            version.to_string(),
                        )))
                    }
                } else if let Some(index) = index {
                    let mut table = toml_edit::Table::new().into_inline_table();
                    // When there is no version, use the star.
                    table.insert(
                        "version",
                        toml_edit::Value::String(toml_edit::Formatted::new("*".to_string())),
                    );
                    table.insert(
                        "index",
                        toml_edit::Value::String(toml_edit::Formatted::new(index.to_string())),
                    );
                    Item::Value(toml_edit::Value::InlineTable(table))
                } else if val.extras.is_some() {
                    // If extras is defined use an inline table
                    let mut table = toml_edit::Table::new().into_inline_table();
                    // First add the version
                    table.insert(
                        "version",
                        toml_edit::Value::String(toml_edit::Formatted::new("*".to_string())),
                    );
                    Item::Value(toml_edit::Value::InlineTable(table))
                } else {
                    // Without extras use the string representation.
                    return Item::Value(toml_edit::Value::String(toml_edit::Formatted::new(
                        "*".to_string(),
                    )));
                }
            }
            PyPiRequirementType::Git {
                git: _,
                branch: _,
                tag: _,
                rev: _,
                subdirectory: _,
            } => {
                todo!("git")
            }
            PyPiRequirementType::Path {
                path: _,
                editable: _,
            } => {
                todo!("path")
            }
            PyPiRequirementType::Url { url: _ } => {
                todo!("url")
            }
        };

        // TODO: extras need to be added to the table.
        if let Some(extras) = val.extras {
            let mut empty_table = toml_edit::Table::new().into_inline_table();
            let table = req_item.as_inline_table_mut().unwrap_or(&mut empty_table);
            table.insert(
                "extras",
                toml_edit::Value::Array(
                    extras
                        .iter()
                        .map(|e| e.to_string())
                        .map(|extra| {
                            toml_edit::Value::String(toml_edit::Formatted::new(extra.clone()))
                        })
                        .collect(),
                ),
            );
            Item::Value(toml_edit::Value::InlineTable(table.to_owned()))
        } else {
            req_item
        }
    }
}

impl FromStr for PyPiRequirement {
    type Err = ParsePyPiRequirementError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // TODO: add sources here
        // From string can only parse the version specifier.
        Ok(Self {
            requirement: PyPiRequirementType::Version(VersionOrStar::from_str(s)?),
            extras: None,
        })
    }
}

/// Implement from [`pep508_rs::Requirement`] to make the conversion easier.
impl From<pep508_rs::Requirement> for PyPiRequirement {
    fn from(req: pep508_rs::Requirement) -> Self {
        let version = if let Some(version_or_url) = req.version_or_url {
            match version_or_url {
                pep508_rs::VersionOrUrl::VersionSpecifier(v) => Some(v),
                pep508_rs::VersionOrUrl::Url(_) => None,
            }
        } else {
            None
        };
        let extras = if !req.extras.is_empty() {
            Some(req.extras)
        } else {
            None
        };
        PyPiRequirement {
            requirement: PyPiRequirementType::Version(VersionOrStar {
                version,
                index: None,
            }),
            extras,
        }
    }
}

impl PyPiRequirement {
    /// Returns the requirements as [`pep508_rs::Requirement`]s.
    pub fn as_pep508(&self, name: &PackageName) -> pep508_rs::Requirement {
        let version_or_url = match &self.requirement {
            PyPiRequirementType::Version(VersionOrStar { version, index: _ }) => version
                .as_ref()
                .map(|v| pep508_rs::VersionOrUrl::VersionSpecifier(v.clone())),
            PyPiRequirementType::Git {
                git: url,
                // TODO: ignoring branch for now
                branch: _,
                tag,
                rev,
                subdirectory: subdir,
            } => {
                // Choose revision over tag if it is specified
                let tag_or_rev = rev.as_ref().or_else(|| tag.as_ref()).cloned();
                // Create the url.
                let url = format!("git+{url}");
                // Add the tag or rev if it exists.
                let url = tag_or_rev
                    .as_ref()
                    .map_or_else(|| url.clone(), |tag_or_rev| format!("{url}@{tag_or_rev}"));
                // Add the subdirectory if it exists.
                let url = subdir.as_ref().map_or_else(
                    || url.clone(),
                    |subdir| format!("{url}#subdirectory={subdir}"),
                );
                Some(pep508_rs::VersionOrUrl::Url(
                    VerbatimUrl::parse(&url).expect("git url is invalid"),
                ))
            }
            PyPiRequirementType::Path { path, editable: _ } => {
                let canonicalized = dunce::canonicalize(path).expect("cannot conoicalize paths");
                let given = path
                    .to_str()
                    .map(|s| s.to_owned())
                    .unwrap_or_else(|| String::new());
                let verbatim = VerbatimUrl::from_path(canonicalized).with_given(given);
                Some(pep508_rs::VersionOrUrl::Url(verbatim))
            }

            PyPiRequirementType::Url { url } => Some(pep508_rs::VersionOrUrl::Url(
                VerbatimUrl::from_url(url.clone()),
            )),
        };
        pep508_rs::Requirement {
            name: name.clone(),
            extras: self.extras.clone().unwrap_or_default(),
            version_or_url,
            marker: None,
        }
    }
}

impl<'de> Deserialize<'de> for PyPiRequirement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .string(|str| PyPiRequirement::from_str(str).map_err(Error::custom))
            .map(|map| {
                // Just use normal deserializer
                #[derive(Deserialize, Debug)]
                struct RawPyPiRequirement {
                    #[serde(flatten)]
                    requirement: Option<PyPiRequirementType>,
                    extras: Option<Vec<String>>,
                }
                let raw_pypi_requirement =
                    RawPyPiRequirement::deserialize(de::value::MapAccessDeserializer::new(map))?;

                let mut extras = None;
                if let Some(raw_extras) = raw_pypi_requirement.extras {
                    extras = Some(
                        raw_extras
                            .into_iter()
                            .map(|e| ExtraName::from_str(&e))
                            .collect::<Result<Vec<ExtraName>, _>>()
                            .map_err(Error::custom)?,
                    );
                }

                Ok(PyPiRequirement {
                    requirement: raw_pypi_requirement.requirement.unwrap_or_default(),
                    extras,
                })
            })
            .expecting("either a map or a string")
            .deserialize(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use std::str::FromStr;

    #[test]
    fn test_pypi_to_string() {
        let req = pep508_rs::Requirement::from_str("numpy[testing]==1.0.0; os_name == \"posix\"")
            .unwrap();
        let pypi = PyPiRequirement::from(req);
        assert_eq!(
            pypi.to_string(),
            "{ version = \"==1.0.0\", extras = [\"testing\"] }"
        );

        let req = pep508_rs::Requirement::from_str("numpy").unwrap();
        let pypi = PyPiRequirement::from(req);
        assert_eq!(pypi.to_string(), "\"*\"");
    }

    #[test]
    fn test_only_version() {
        let requirement: IndexMap<uv_normalize::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = ">=3.12""#).unwrap();
        assert_eq!(
            requirement.first().unwrap().0,
            &uv_normalize::PackageName::from_str("foo").unwrap()
        );
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Version(VersionOrStar {
                    version: Some(pep440_rs::VersionSpecifiers::from_str(">=3.12").unwrap()),
                    index: None,
                }),
                ..PyPiRequirement::default()
            }
        );
        let requirement: IndexMap<uv_normalize::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = "==3.12.0""#).unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Version(VersionOrStar {
                    version: Some(pep440_rs::VersionSpecifiers::from_str("==3.12.0").unwrap()),
                    index: None,
                }),
                ..PyPiRequirement::default()
            }
        );

        let requirement: IndexMap<uv_normalize::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = "~=2.1.3""#).unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Version(VersionOrStar {
                    version: Some(pep440_rs::VersionSpecifiers::from_str("~=2.1.3").unwrap()),
                    index: None,
                }),
                ..PyPiRequirement::default()
            }
        );

        let requirement: IndexMap<uv_normalize::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = "*""#).unwrap();
        assert_eq!(requirement.first().unwrap().1, &PyPiRequirement::default());
    }

    #[test]
    fn test_extended() {
        let requirement: IndexMap<uv_normalize::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(
                r#"
                foo = { version=">=3.12", extras = ["bar"], index = "artifact-registry" }
                "#,
            )
            .unwrap();

        assert_eq!(
            requirement.first().unwrap().0,
            &uv_normalize::PackageName::from_str("foo").unwrap()
        );
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Version(VersionOrStar {
                    version: Some(pep440_rs::VersionSpecifiers::from_str(">=3.12").unwrap()),
                    index: Some("artifact-registry".to_string()),
                }),
                extras: Some(vec![ExtraName::from_str("bar").unwrap()]),
            }
        );

        let requirement: IndexMap<uv_normalize::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(
                r#"bar = { version=">=3.12,<3.13.0", extras = ["bar", "foo"] }"#,
            )
            .unwrap();
        assert_eq!(
            requirement.first().unwrap().0,
            &uv_normalize::PackageName::from_str("bar").unwrap()
        );
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Version(VersionOrStar {
                    version: Some(
                        pep440_rs::VersionSpecifiers::from_str(">=3.12,<3.13.0").unwrap()
                    ),
                    index: None,
                }),
                extras: Some(vec![
                    ExtraName::from_str("bar").unwrap(),
                    ExtraName::from_str("foo").unwrap(),
                ]),
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_requirement_from_map() {
        let json_string = r#"
            {
                "version": "==1.2.3",
                "extras": ["feature1", "feature2"]
            }
        "#;
        let result: Result<PyPiRequirement, _> = serde_json::from_str(json_string);
        assert!(result.is_ok());
        let pypi_requirement: PyPiRequirement = result.unwrap();
        assert_eq!(
            pypi_requirement,
            PyPiRequirement {
                requirement: PyPiRequirementType::Version(VersionOrStar {
                    version: Some(pep440_rs::VersionSpecifiers::from_str("==1.2.3").unwrap()),
                    index: None
                }),
                extras: Some(vec![
                    ExtraName::from_str("feature1").unwrap(),
                    ExtraName::from_str("feature2").unwrap()
                ]),
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_requirement_from_str() {
        let json_string = r#""==1.2.3""#;
        let result: Result<PyPiRequirement, _> = serde_json::from_str(json_string);
        assert!(result.is_ok());
        let pypi_requirement: PyPiRequirement = result.unwrap();
        assert_eq!(
            pypi_requirement,
            PyPiRequirement {
                requirement: PyPiRequirementType::Version(VersionOrStar {
                    version: Some(pep440_rs::VersionSpecifiers::from_str("==1.2.3").unwrap()),
                    index: None,
                }),
                ..PyPiRequirement::default()
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_requirement_from_str_with_star() {
        let json_string = r#""*""#;
        let result: Result<PyPiRequirement, _> = serde_json::from_str(json_string);
        assert!(result.is_ok());
        let pypi_requirement: PyPiRequirement = result.unwrap();
        assert_eq!(pypi_requirement, PyPiRequirement::default());
    }

    #[test]
    fn test_deserialize_pypi_from_path() {
        let requirement: IndexMap<PyPiPackageName, PyPiRequirement> = toml_edit::de::from_str(
            r#"
                foo = { path = "../numpy-test" }
                "#,
        )
        .unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Path {
                    path: PathBuf::from("../numpy-test"),
                    editable: None,
                },
                ..PyPiRequirement::default()
            }
        );
    }
    #[test]
    fn test_deserialize_pypi_from_path_editable() {
        let requirement: IndexMap<PyPiPackageName, PyPiRequirement> = toml_edit::de::from_str(
            r#"
                foo = { path = "../numpy-test", editable = true }
                "#,
        )
        .unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Path {
                    path: PathBuf::from("../numpy-test"),
                    editable: Some(true),
                },
                extras: None,
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_from_url() {
        let requirement: IndexMap<PyPiPackageName, PyPiRequirement> = toml_edit::de::from_str(
            r#"
                foo = { url = "https://test.url.com"}
                "#,
        )
        .unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Url {
                    url: Url::parse("https://test.url.com").unwrap()
                },
                ..PyPiRequirement::default()
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_from_git() {
        let requirement: IndexMap<PyPiPackageName, PyPiRequirement> = toml_edit::de::from_str(
            r#"
                foo = { git = "https://test.url.git" }
                "#,
        )
        .unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Git {
                    git: Url::parse("https://test.url.git").unwrap(),
                    branch: None,
                    tag: None,
                    rev: None,
                    subdirectory: None,
                },
                ..PyPiRequirement::default()
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_from_git_branch() {
        let requirement: IndexMap<PyPiPackageName, PyPiRequirement> = toml_edit::de::from_str(
            r#"
                foo = { git = "https://test.url.git", branch = "main" }
                "#,
        )
        .unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Git {
                    git: Url::parse("https://test.url.git").unwrap(),
                    branch: Some("main".to_string()),
                    tag: None,
                    rev: None,
                    subdirectory: None,
                },
                ..PyPiRequirement::default()
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_from_git_tag() {
        let requirement: IndexMap<PyPiPackageName, PyPiRequirement> = toml_edit::de::from_str(
            r#"
                foo = { git = "https://test.url.git", tag = "v.1.2.3" }
                "#,
        )
        .unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Git {
                    git: Url::parse("https://test.url.git").unwrap(),
                    tag: Some("v.1.2.3".to_string()),
                    branch: None,
                    rev: None,
                    subdirectory: None,
                },
                ..PyPiRequirement::default()
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_from_flask() {
        let requirement: IndexMap<PyPiPackageName, PyPiRequirement> = toml_edit::de::from_str(
            r#"
                flask = { git = "https://github.com/pallets/flask.git", tag = "3.0.0"}
                "#,
        )
        .unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Git {
                    git: Url::parse("https://github.com/pallets/flask.git").unwrap(),
                    tag: Some("3.0.0".to_string()),
                    branch: None,
                    rev: None,
                    subdirectory: None,
                },
                ..PyPiRequirement::default()
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_from_git_rev() {
        let requirement: IndexMap<PyPiPackageName, PyPiRequirement> = toml_edit::de::from_str(
            r#"
                foo = { git = "https://test.url.git", rev = "123456" }
                "#,
        )
        .unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                requirement: PyPiRequirementType::Git {
                    git: Url::parse("https://test.url.git").unwrap(),
                    rev: Some("123456".to_string()),
                    tag: None,
                    branch: None,
                    subdirectory: None,
                },
                ..PyPiRequirement::default()
            }
        );
    }
}
