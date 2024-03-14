use pep440_rs::VersionSpecifiers;
use pep508_rs::VerbatimUrl;
use serde::Serializer;
use serde::{de::Error, Deserialize, Deserializer, Serialize};
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

/// The pep crate does not support "*" as a version specifier, so we need to
/// handle it ourselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionOrStar {
    Version(VersionSpecifiers),
    Star,
}

impl FromStr for VersionOrStar {
    type Err = pep440_rs::VersionSpecifiersParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "*" {
            Ok(VersionOrStar::Star)
        } else {
            Ok(VersionOrStar::Version(VersionSpecifiers::from_str(s)?))
        }
    }
}

impl ToString for VersionOrStar {
    fn to_string(&self) -> String {
        match self {
            VersionOrStar::Version(v) => v.to_string(),
            VersionOrStar::Star => "*".to_string(),
        }
    }
}

impl From<VersionOrStar> for Option<pep508_rs::VersionOrUrl> {
    fn from(val: VersionOrStar) -> Self {
        match val {
            VersionOrStar::Version(v) => Some(pep508_rs::VersionOrUrl::VersionSpecifier(v)),
            VersionOrStar::Star => None,
        }
    }
}

// Custom serialization function
fn serialize_version_or_star<S>(value: &VersionOrStar, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&value.to_string())
}

// Custom deserialization function
fn deserialize_version_or_star<'de, D>(deserializer: D) -> Result<VersionOrStar, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    VersionOrStar::from_str(&s).map_err(serde::de::Error::custom)
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(untagged, rename_all = "snake_case")]
pub enum PyPiRequirement {
    Git {
        git: Url,
        branch: Option<String>,
        tag: Option<String>,
        rev: Option<String>,
        subdirectory: Option<String>,
        #[serde(default)]
        extras: Vec<ExtraName>,
    },
    Path {
        path: PathBuf,
        editable: Option<bool>,
        #[serde(default)]
        extras: Vec<ExtraName>,
    },
    Url {
        url: Url,
        #[serde(default)]
        extras: Vec<ExtraName>,
    },
    Version {
        #[serde(
            serialize_with = "serialize_version_or_star",
            deserialize_with = "deserialize_version_or_star"
        )]
        version: VersionOrStar,
        index: Option<String>,
        #[serde(default)]
        extras: Vec<ExtraName>,
    },
    RawVersion(
        #[serde(
            serialize_with = "serialize_version_or_star",
            deserialize_with = "deserialize_version_or_star"
        )]
        VersionOrStar,
    ),
}

impl Default for PyPiRequirement {
    fn default() -> Self {
        PyPiRequirement::RawVersion(VersionOrStar::Star)
    }
}

/// The type of parse error that occurred when parsing match spec.
#[derive(Debug, Clone, Error)]
pub enum ParsePyPiRequirementError {
    #[error("invalid pep440 version specifier")]
    Pep440Error(#[from] pep440_rs::VersionSpecifiersParseError),
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
        fn insert_extras(table: &mut toml_edit::InlineTable, extras: &[ExtraName]) {
            if !extras.is_empty() {
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
            }
        }

        match &val {
            PyPiRequirement::Version {
                version,
                index,
                extras,
            } => {
                let mut table = toml_edit::Table::new().into_inline_table();
                table.insert(
                    "version",
                    toml_edit::Value::String(toml_edit::Formatted::new(version.to_string())),
                );
                if let Some(index) = index {
                    table.insert(
                        "index",
                        toml_edit::Value::String(toml_edit::Formatted::new(index.to_string())),
                    );
                }
                insert_extras(&mut table, extras);
                Item::Value(toml_edit::Value::InlineTable(table.to_owned()))
            }
            PyPiRequirement::Git {
                git: _,
                branch: _,
                tag: _,
                rev: _,
                subdirectory: _,
                extras: _,
            } => {
                unimplemented!("git")
            }
            PyPiRequirement::Path {
                path: _,
                editable: _,
                extras: _,
            } => {
                unimplemented!("path")
            }
            PyPiRequirement::Url { url: _, extras: _ } => {
                unimplemented!("url")
            }
            PyPiRequirement::RawVersion(version) => Item::Value(toml_edit::Value::String(
                toml_edit::Formatted::new(version.to_string()),
            )),
        }
    }
}

/// Implement from [`pep508_rs::Requirement`] to make the conversion easier.
impl From<pep508_rs::Requirement> for PyPiRequirement {
    fn from(req: pep508_rs::Requirement) -> Self {
        if let Some(version_or_url) = req.version_or_url {
            match version_or_url {
                pep508_rs::VersionOrUrl::VersionSpecifier(v) => PyPiRequirement::Version {
                    version: VersionOrStar::Version(v),
                    index: None,
                    extras: req.extras,
                },
                pep508_rs::VersionOrUrl::Url(u) => PyPiRequirement::Url {
                    url: u.to_url(),
                    extras: req.extras,
                },
            }
        } else {
            if !req.extras.is_empty() {
                PyPiRequirement::Version {
                    version: VersionOrStar::Star,
                    index: None,
                    extras: req.extras,
                }
            } else {
                PyPiRequirement::RawVersion(VersionOrStar::Star)
            }
        }
    }
}

impl PyPiRequirement {
    pub fn extras(&self) -> &[ExtraName] {
        match self {
            PyPiRequirement::Version { extras, .. } => extras,
            PyPiRequirement::Git { extras, .. } => extras,
            PyPiRequirement::Path { extras, .. } => extras,
            PyPiRequirement::Url { extras, .. } => extras,
            PyPiRequirement::RawVersion(_) => &[],
        }
    }

    /// Returns the requirements as [`pep508_rs::Requirement`]s.
    pub fn as_pep508(&self, name: &PackageName) -> pep508_rs::Requirement {
        let version_or_url = match self {
            PyPiRequirement::Version {
                version,
                index: _,
                extras: _,
            } => version.clone().into(),
            PyPiRequirement::Git {
                git,
                branch: _,
                tag: _,
                rev: _,
                subdirectory: _,
                extras: _,
            } => Some(pep508_rs::VersionOrUrl::Url(VerbatimUrl::from_url(
                git.clone(),
            ))),
            PyPiRequirement::Path {
                path: _,
                editable: _,
                extras: _,
            } => {
                unimplemented!("No path to url conversion yet.")
            }
            PyPiRequirement::Url { url, extras: _ } => Some(pep508_rs::VersionOrUrl::Url(
                VerbatimUrl::from_url(url.clone()),
            )),
            PyPiRequirement::RawVersion(version) => version.clone().into(),
        };

        pep508_rs::Requirement {
            name: name.clone(),
            extras: self.extras().to_vec(),
            version_or_url,
            marker: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use insta::assert_snapshot;
    use pep508_rs::Requirement;
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
            &PyPiRequirement::RawVersion(">=3.12".parse().unwrap())
        );

        let requirement: IndexMap<uv_normalize::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = "==3.12.0""#).unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::RawVersion("==3.12.0".parse().unwrap())
        );

        let requirement: IndexMap<uv_normalize::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = "~=2.1.3""#).unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::RawVersion("~=2.1.3".parse().unwrap())
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
            &PyPiRequirement::Version {
                version: ">=3.12".parse().unwrap(),
                index: Some("artifact-registry".to_string()),
                extras: vec![ExtraName::from_str("bar").unwrap()],
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
            &PyPiRequirement::Version {
                version: ">=3.12,<3.13.0".parse().unwrap(),
                index: None,
                extras: vec![
                    ExtraName::from_str("bar").unwrap(),
                    ExtraName::from_str("foo").unwrap(),
                ],
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
            PyPiRequirement::Version {
                version: "==1.2.3".parse().unwrap(),
                index: None,
                extras: vec![
                    ExtraName::from_str("feature1").unwrap(),
                    ExtraName::from_str("feature2").unwrap()
                ],
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
            PyPiRequirement::RawVersion("==1.2.3".parse().unwrap())
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
            &PyPiRequirement::Path {
                path: PathBuf::from("../numpy-test"),
                editable: None,
                extras: vec![],
            },
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
            &PyPiRequirement::Path {
                path: PathBuf::from("../numpy-test"),
                editable: Some(true),
                extras: vec![],
            }
        );
    }

    #[test]
    fn test_deserialize_fail_on_unknown() {
        let _ = toml_edit::de::from_str::<IndexMap<PyPiPackageName, PyPiRequirement>>(
            r#"foo = { borked = "bork"}"#,
        )
        .unwrap_err();
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
            &PyPiRequirement::Url {
                url: Url::parse("https://test.url.com").unwrap(),
                extras: vec![]
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
            &PyPiRequirement::Git {
                git: Url::parse("https://test.url.git").unwrap(),
                branch: None,
                tag: None,
                rev: None,
                subdirectory: None,
                extras: vec![],
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
            &PyPiRequirement::Git {
                git: Url::parse("https://test.url.git").unwrap(),
                branch: Some("main".to_string()),
                tag: None,
                rev: None,
                subdirectory: None,
                extras: vec![],
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
            &PyPiRequirement::Git {
                git: Url::parse("https://test.url.git").unwrap(),
                tag: Some("v.1.2.3".to_string()),
                branch: None,
                rev: None,
                subdirectory: None,
                extras: vec![],
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
            &PyPiRequirement::Git {
                git: Url::parse("https://test.url.git").unwrap(),
                rev: Some("123456".to_string()),
                tag: None,
                branch: None,
                subdirectory: None,
                extras: vec![],
            }
        );
    }

    #[test]
    fn test_from_args() {
        let pypi : Requirement = "numpy".parse().unwrap();
        let as_pypi_req : PyPiRequirement = pypi.into();
        // convert to toml and snapshot
        assert_snapshot!(as_pypi_req.to_string());

        let pypi : Requirement = "numpy[test,extrastuff]".parse().unwrap();
        let as_pypi_req : PyPiRequirement = pypi.into();
        // convert to toml and snapshot
        assert_snapshot!(as_pypi_req.to_string());
    }
}
