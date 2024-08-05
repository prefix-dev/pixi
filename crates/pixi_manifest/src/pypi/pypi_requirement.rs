use std::{fmt, fmt::Formatter, path::PathBuf, str::FromStr};

use super::{pypi_requirement_types::GitRevParseError, GitRev, VersionOrStar};
use crate::utils::extract_directory_from_url;
use pep508_rs::ExtraName;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(untagged, rename_all = "snake_case", deny_unknown_fields)]
pub enum PyPiRequirement {
    Git {
        git: Url,
        branch: Option<String>,
        tag: Option<String>,
        rev: Option<GitRev>,
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
        subdirectory: Option<String>,
        #[serde(default)]
        extras: Vec<ExtraName>,
    },
    Version {
        version: VersionOrStar,
        #[serde(default)]
        extras: Vec<ExtraName>,
    },
    RawVersion(VersionOrStar),
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
        let toml = toml_edit::Value::from(self.clone());
        write!(f, "{toml}")
    }
}

impl From<PyPiRequirement> for toml_edit::Value {
    /// PyPiRequirement to a toml_edit item, to put in the manifest file.
    fn from(val: PyPiRequirement) -> toml_edit::Value {
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
            PyPiRequirement::Version { version, extras } if extras.is_empty() => {
                toml_edit::Value::from(version.to_string())
            }
            PyPiRequirement::Version { version, extras } => {
                let mut table = toml_edit::Table::new().into_inline_table();
                table.insert(
                    "version",
                    toml_edit::Value::String(toml_edit::Formatted::new(version.to_string())),
                );
                insert_extras(&mut table, extras);
                toml_edit::Value::InlineTable(table.to_owned())
            }
            PyPiRequirement::Git {
                git,
                branch,
                tag,
                rev,
                subdirectory: _,
                extras,
            } => {
                let mut table = toml_edit::Table::new().into_inline_table();
                table.insert(
                    "git",
                    toml_edit::Value::String(toml_edit::Formatted::new(git.to_string())),
                );
                if let Some(branch) = branch {
                    table.insert(
                        "branch",
                        toml_edit::Value::String(toml_edit::Formatted::new(branch.clone())),
                    );
                }
                if let Some(tag) = tag {
                    table.insert(
                        "tag",
                        toml_edit::Value::String(toml_edit::Formatted::new(tag.clone())),
                    );
                }
                if let Some(rev) = rev {
                    table.insert(
                        "rev",
                        toml_edit::Value::String(toml_edit::Formatted::new(rev.to_string())),
                    );
                }
                insert_extras(&mut table, extras);
                toml_edit::Value::InlineTable(table.to_owned())
            }
            PyPiRequirement::Path {
                path,
                editable,
                extras,
            } => {
                let mut table = toml_edit::Table::new().into_inline_table();
                table.insert(
                    "path",
                    toml_edit::Value::String(toml_edit::Formatted::new(
                        path.to_string_lossy().to_string(),
                    )),
                );
                if editable == &Some(true) {
                    table.insert(
                        "editable",
                        toml_edit::Value::Boolean(toml_edit::Formatted::new(true)),
                    );
                }
                insert_extras(&mut table, extras);
                toml_edit::Value::InlineTable(table.to_owned())
            }
            PyPiRequirement::Url {
                url,
                extras,
                subdirectory,
            } => {
                let mut table = toml_edit::Table::new().into_inline_table();
                table.insert(
                    "url",
                    toml_edit::Value::String(toml_edit::Formatted::new(url.to_string())),
                );
                if let Some(subdirectory) = subdirectory {
                    table.insert(
                        "subdirectory",
                        toml_edit::Value::String(toml_edit::Formatted::new(
                            subdirectory.to_string(),
                        )),
                    );
                }
                insert_extras(&mut table, extras);
                toml_edit::Value::InlineTable(table.to_owned())
            }
            PyPiRequirement::RawVersion(version) => {
                toml_edit::Value::String(toml_edit::Formatted::new(version.to_string()))
            }
        }
    }
}

#[derive(Error, Clone, Debug)]
pub enum Pep508ToPyPiRequirementError {
    #[error(transparent)]
    ParseUrl(#[from] url::ParseError),

    #[error(transparent)]
    ParseGitRev(#[from] GitRevParseError),

    #[error("could not convert '{0}' to a file path")]
    PathUrlIntoPath(Url),
}

/// Implement from [`pep508_rs::Requirement`] to make the conversion easier.
impl TryFrom<pep508_rs::Requirement> for PyPiRequirement {
    type Error = Pep508ToPyPiRequirementError;
    fn try_from(req: pep508_rs::Requirement) -> Result<Self, Self::Error> {
        let converted = if let Some(version_or_url) = req.version_or_url {
            match version_or_url {
                pep508_rs::VersionOrUrl::VersionSpecifier(v) => PyPiRequirement::Version {
                    version: if v.is_empty() {
                        VersionOrStar::Star
                    } else {
                        VersionOrStar::Version(v)
                    },
                    extras: req.extras,
                },
                pep508_rs::VersionOrUrl::Url(u) => {
                    // If serialization starts with `git+` then it is a git url.
                    if let Some(stripped_url) = u.to_string().strip_prefix("git+") {
                        if let Some((url, version)) = stripped_url.split_once('@') {
                            let url = Url::parse(url)?;
                            PyPiRequirement::Git {
                                git: url,
                                branch: None,
                                tag: None,
                                rev: Some(GitRev::from_str(version)?),
                                subdirectory: None,
                                extras: req.extras,
                            }
                        } else {
                            let url = Url::parse(stripped_url)?;
                            PyPiRequirement::Git {
                                git: url,
                                branch: None,
                                tag: None,
                                rev: None,
                                subdirectory: None,
                                extras: req.extras,
                            }
                        }
                    } else {
                        let url = u.to_url();
                        // Have a different code path when the url is a file.
                        // i.e. package @ file:///path/to/package
                        if url.scheme() == "file" {
                            // Convert the file url to a path.
                            let file = url.to_file_path().map_err(|_| {
                                Pep508ToPyPiRequirementError::PathUrlIntoPath(url.clone())
                            })?;
                            PyPiRequirement::Path {
                                path: file,
                                editable: None,
                                extras: req.extras,
                            }
                        } else {
                            let subdirectory = extract_directory_from_url(&url);
                            PyPiRequirement::Url {
                                url,
                                extras: req.extras,
                                subdirectory,
                            }
                        }
                    }
                }
            }
        } else if !req.extras.is_empty() {
            PyPiRequirement::Version {
                version: VersionOrStar::Star,
                extras: req.extras,
            }
        } else {
            PyPiRequirement::RawVersion(VersionOrStar::Star)
        };
        Ok(converted)
    }
}

impl PyPiRequirement {
    /// Returns true if the requirement is a direct dependency.
    /// I.e. a url, path or git requirement.
    pub fn is_direct_dependency(&self) -> bool {
        matches!(
            self,
            PyPiRequirement::Git { .. }
                | PyPiRequirement::Path { .. }
                | PyPiRequirement::Url { .. }
        )
    }

    /// Define whether the requirement is editable.
    pub fn set_editable(&mut self, editable: bool) {
        match self {
            PyPiRequirement::Path { editable: e, .. } => {
                *e = Some(editable);
            }
            _ if editable => {
                tracing::warn!("Ignoring editable flag for non-path requirements.");
            }
            _ => {}
        }
    }

    pub fn extras(&self) -> &[ExtraName] {
        match self {
            PyPiRequirement::Version { extras, .. } => extras,
            PyPiRequirement::Git { extras, .. } => extras,
            PyPiRequirement::Path { extras, .. } => extras,
            PyPiRequirement::Url { extras, .. } => extras,
            PyPiRequirement::RawVersion(_) => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::pypi::PyPiPackageName;
    use indexmap::IndexMap;
    use insta::assert_snapshot;
    use pep508_rs::Requirement;

    #[test]
    fn test_pypi_to_string() {
        let req = pep508_rs::Requirement::from_str("numpy[testing]==1.0.0; os_name == \"posix\"")
            .unwrap();
        let pypi = PyPiRequirement::try_from(req).unwrap();
        assert_eq!(
            pypi.to_string(),
            "{ version = \"==1.0.0\", extras = [\"testing\"] }"
        );

        let req = pep508_rs::Requirement::from_str("numpy").unwrap();
        let pypi = PyPiRequirement::try_from(req).unwrap();
        assert_eq!(pypi.to_string(), "\"*\"");
    }

    #[test]
    fn test_only_version() {
        let requirement: IndexMap<pep508_rs::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = ">=3.12""#).unwrap();
        assert_eq!(
            requirement.first().unwrap().0,
            &pep508_rs::PackageName::from_str("foo").unwrap()
        );
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::RawVersion(">=3.12".parse().unwrap())
        );

        let requirement: IndexMap<pep508_rs::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = "==3.12.0""#).unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::RawVersion("==3.12.0".parse().unwrap())
        );

        let requirement: IndexMap<pep508_rs::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = "~=2.1.3""#).unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::RawVersion("~=2.1.3".parse().unwrap())
        );

        let requirement: IndexMap<pep508_rs::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = "*""#).unwrap();
        assert_eq!(requirement.first().unwrap().1, &PyPiRequirement::default());
    }

    #[test]
    fn test_extended() {
        let requirement: IndexMap<pep508_rs::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(
                r#"
                    foo = { version=">=3.12", extras = ["bar"]}
                    "#,
            )
            .unwrap();

        assert_eq!(
            requirement.first().unwrap().0,
            &pep508_rs::PackageName::from_str("foo").unwrap()
        );
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::Version {
                version: ">=3.12".parse().unwrap(),
                extras: vec![ExtraName::from_str("bar").unwrap()],
            }
        );

        let requirement: IndexMap<pep508_rs::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(
                r#"bar = { version=">=3.12,<3.13.0", extras = ["bar", "foo"] }"#,
            )
            .unwrap();
        assert_eq!(
            requirement.first().unwrap().0,
            &pep508_rs::PackageName::from_str("bar").unwrap()
        );
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::Version {
                version: ">=3.12,<3.13.0".parse().unwrap(),
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
                extras: vec![],
                subdirectory: None,
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
    fn test_deserialize_pypi_from_flask() {
        let requirement: IndexMap<PyPiPackageName, PyPiRequirement> = toml_edit::de::from_str(
            r#"
                flask = { git = "https://github.com/pallets/flask.git", tag = "3.0.0"}
                "#,
        )
        .unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::Git {
                git: Url::parse("https://github.com/pallets/flask.git").unwrap(),
                tag: Some("3.0.0".to_string()),
                branch: None,
                rev: None,
                subdirectory: None,
                extras: vec![],
            },
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
                rev: Some(GitRev::Short("123456".to_string())),
                tag: None,
                branch: None,
                subdirectory: None,
                extras: vec![],
            }
        );
    }

    #[test]
    fn test_from_args() {
        let pypi: Requirement = "numpy".parse().unwrap();
        let as_pypi_req: PyPiRequirement = pypi.try_into().unwrap();
        // convert to toml and snapshot
        assert_snapshot!(as_pypi_req.to_string());

        let pypi: Requirement = "numpy[test,extrastuff]".parse().unwrap();
        let as_pypi_req: PyPiRequirement = pypi.try_into().unwrap();
        // convert to toml and snapshot
        assert_snapshot!(as_pypi_req.to_string());

        let pypi: Requirement = "exchangelib @ git+https://github.com/ecederstrand/exchangelib"
            .parse()
            .unwrap();
        let as_pypi_req: PyPiRequirement = pypi.try_into().unwrap();
        assert_eq!(
            as_pypi_req,
            PyPiRequirement::Git {
                git: Url::parse("https://github.com/ecederstrand/exchangelib").unwrap(),
                branch: None,
                tag: None,
                rev: None,
                subdirectory: None,
                extras: vec![]
            }
        );

        let pypi: Requirement = "exchangelib @ git+https://github.com/ecederstrand/exchangelib@b283011c6df4a9e034baca9aea19aa8e5a70e3ab".parse().unwrap();
        let as_pypi_req: PyPiRequirement = pypi.try_into().unwrap();
        assert_eq!(
            as_pypi_req,
            PyPiRequirement::Git {
                git: Url::parse("https://github.com/ecederstrand/exchangelib").unwrap(),
                branch: None,
                tag: None,
                rev: Some(GitRev::Full(
                    "b283011c6df4a9e034baca9aea19aa8e5a70e3ab".to_string()
                )),
                subdirectory: None,
                extras: vec![]
            }
        );

        let pypi: Requirement = "boltons @ https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl".parse().unwrap();
        let as_pypi_req: PyPiRequirement = pypi.try_into().unwrap();
        assert_eq!(as_pypi_req, PyPiRequirement::Url{url: Url::parse("https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl").unwrap(), extras: vec![], subdirectory: None });

        let pypi: Requirement = "boltons[nichita] @ https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl".parse().unwrap();
        let as_pypi_req: PyPiRequirement = pypi.try_into().unwrap();
        assert_eq!(as_pypi_req, PyPiRequirement::Url{url: Url::parse("https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl").unwrap(), extras: vec![ExtraName::new("nichita".to_string()).unwrap()], subdirectory: None });

        #[cfg(target_os = "windows")]
        let pypi: Requirement = "boltons @ file:///C:/path/to/boltons".parse().unwrap();
        #[cfg(not(target_os = "windows"))]
        let pypi: Requirement = "boltons @ file:///path/to/boltons".parse().unwrap();

        let as_pypi_req: PyPiRequirement = pypi.try_into().unwrap();

        #[cfg(target_os = "windows")]
        assert_eq!(
            as_pypi_req,
            PyPiRequirement::Path {
                path: PathBuf::from("C:/path/to/boltons"),
                editable: None,
                extras: vec![]
            }
        );
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            as_pypi_req,
            PyPiRequirement::Path {
                path: PathBuf::from("/path/to/boltons"),
                editable: None,
                extras: vec![]
            }
        );
    }
}
