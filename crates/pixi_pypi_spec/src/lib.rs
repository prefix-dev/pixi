mod name;
mod pep508;
mod toml;
pub mod utils;
mod version_or_star;

use std::{
    fmt::{self, Formatter},
    path::PathBuf,
};

use pep440_rs::VersionSpecifiers;
use pep508_rs::ExtraName;
use pixi_spec::GitSpec;
use serde::Serialize;
use thiserror::Error;
use url::Url;

pub use name::PypiPackageName;
pub use version_or_star::VersionOrStar;

/// A representation of a PyPI requirement specifier used in pixi.
#[derive(Debug, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(untagged, rename_all = "kebab-case", deny_unknown_fields)]
pub enum PixiPypiSpec {
    Git {
        url: GitSpec,
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
        #[serde(default)]
        index: Option<Url>,
    },
    RawVersion(VersionOrStar),
}

impl Default for PixiPypiSpec {
    fn default() -> Self {
        PixiPypiSpec::RawVersion(VersionOrStar::Star)
    }
}

/// The type of parse error that occurred when parsing match spec.
#[derive(Debug, Clone, Error)]
pub enum ParsePyPiRequirementError {
    #[error("invalid pep440 version specifier")]
    Pep440Error(#[from] pep440_rs::VersionSpecifiersParseError),
}

impl fmt::Display for PixiPypiSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let toml = toml_edit::Value::from(self.clone());
        write!(f, "{toml}")
    }
}

#[derive(Error, Clone, Debug)]
pub enum Pep508ToPyPiRequirementError {
    #[error(transparent)]
    ParseUrl(#[from] url::ParseError),

    #[error("could not convert '{0}' to a file path")]
    PathUrlIntoPath(Url),

    #[error("Unsupported URL prefix `{prefix}` in Url: `{url}` ({message})")]
    UnsupportedUrlPrefix {
        prefix: String,
        url: Url,
        message: &'static str,
    },
}

impl From<VersionSpecifiers> for VersionOrStar {
    fn from(value: VersionSpecifiers) -> Self {
        if value.is_empty() {
            VersionOrStar::Star
        } else {
            VersionOrStar::Version(value)
        }
    }
}

impl PixiPypiSpec {
    /// Returns true if the requirement is a direct dependency.
    /// I.e. a url, path or git requirement.
    pub fn is_direct_dependency(&self) -> bool {
        matches!(
            self,
            PixiPypiSpec::Git { .. } | PixiPypiSpec::Path { .. } | PixiPypiSpec::Url { .. }
        )
    }

    /// Returns the path of the requirement if it is a path requirement.
    pub fn as_path(&self) -> Option<&PathBuf> {
        match self {
            PixiPypiSpec::Path { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Returns the git spec of the requirement if it is a git requirement.
    pub fn as_git(&self) -> Option<&GitSpec> {
        match self {
            PixiPypiSpec::Git { url, .. } => Some(url),
            _ => None,
        }
    }

    /// Returns the url of the requirement if it is a url requirement.
    pub fn as_url(&self) -> Option<&Url> {
        match self {
            PixiPypiSpec::Url { url, .. } => Some(url),
            _ => None,
        }
    }

    /// Returns the version of the requirement if it is a version requirement.
    pub fn as_version(&self) -> Option<&VersionOrStar> {
        match self {
            PixiPypiSpec::Version { version, .. } | PixiPypiSpec::RawVersion(version) => {
                Some(version)
            }
            _ => None,
        }
    }

    /// Define whether the requirement is editable.
    pub fn set_editable(&mut self, editable: bool) {
        match self {
            PixiPypiSpec::Path { editable: e, .. } => {
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
            PixiPypiSpec::Version { extras, .. } => extras,
            PixiPypiSpec::Git { extras, .. } => extras,
            PixiPypiSpec::Path { extras, .. } => extras,
            PixiPypiSpec::Url { extras, .. } => extras,
            PixiPypiSpec::RawVersion(_) => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use insta::assert_snapshot;
    use itertools::Itertools;
    use pep508_rs::Requirement;
    use pixi_spec::GitReference;
    use pixi_test_utils::format_parse_error;
    use pixi_toml::TomlDiagnostic;
    use serde_json::{Value, json};
    use toml_span::de_helpers::TableHelper;

    #[test]
    fn test_pypi_to_string() {
        let req = pep508_rs::Requirement::from_str("numpy[testing]==1.0.0; os_name == \"posix\"")
            .unwrap();
        let pypi = PixiPypiSpec::try_from(req).unwrap();
        assert_eq!(
            pypi.to_string(),
            "{ version = \"==1.0.0\", extras = [\"testing\"] }"
        );

        let req = pep508_rs::Requirement::from_str("numpy").unwrap();
        let pypi = PixiPypiSpec::try_from(req).unwrap();
        assert_eq!(pypi.to_string(), "\"*\"");
    }

    #[test]
    fn test_from_args() {
        let pypi: Requirement = "numpy".parse().unwrap();
        let as_pypi_req: PixiPypiSpec = pypi.try_into().unwrap();
        // convert to toml and snapshot
        assert_snapshot!(as_pypi_req.to_string());

        let pypi: Requirement = "numpy[test,extrastuff]".parse().unwrap();
        let as_pypi_req: PixiPypiSpec = pypi.try_into().unwrap();
        // convert to toml and snapshot
        assert_snapshot!(as_pypi_req.to_string());

        let pypi: Requirement = "exchangelib @ git+https://github.com/ecederstrand/exchangelib"
            .parse()
            .unwrap();
        let as_pypi_req: PixiPypiSpec = pypi.try_into().unwrap();
        assert_eq!(
            as_pypi_req,
            PixiPypiSpec::Git {
                url: GitSpec {
                    git: Url::parse("https://github.com/ecederstrand/exchangelib").unwrap(),
                    rev: Some(GitReference::DefaultBranch),
                    subdirectory: None,
                },
                extras: vec![]
            }
        );

        let pypi: Requirement = "exchangelib @ git+https://github.com/ecederstrand/exchangelib@b283011c6df4a9e034baca9aea19aa8e5a70e3ab".parse().unwrap();
        let as_pypi_req: PixiPypiSpec = pypi.try_into().unwrap();
        assert_eq!(
            as_pypi_req,
            PixiPypiSpec::Git {
                url: GitSpec {
                    git: Url::parse("https://github.com/ecederstrand/exchangelib").unwrap(),
                    rev: Some(GitReference::Rev(
                        "b283011c6df4a9e034baca9aea19aa8e5a70e3ab".to_string()
                    )),
                    subdirectory: None,
                },
                extras: vec![]
            }
        );

        let pypi: Requirement = "boltons @ https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl".parse().unwrap();
        let as_pypi_req: PixiPypiSpec = pypi.try_into().unwrap();
        assert_eq!(as_pypi_req, PixiPypiSpec::Url { url: Url::parse("https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl").unwrap(), extras: vec![], subdirectory: None });

        let pypi: Requirement = "boltons[nichita] @ https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl".parse().unwrap();
        let as_pypi_req: PixiPypiSpec = pypi.try_into().unwrap();
        assert_eq!(as_pypi_req, PixiPypiSpec::Url { url: Url::parse("https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl").unwrap(), extras: vec![ExtraName::new("nichita".to_string()).unwrap()], subdirectory: None });

        #[cfg(target_os = "windows")]
        let pypi: Requirement = "boltons @ file:///C:/path/to/boltons".parse().unwrap();
        #[cfg(not(target_os = "windows"))]
        let pypi: Requirement = "boltons @ file:///path/to/boltons".parse().unwrap();

        let as_pypi_req: PixiPypiSpec = pypi.try_into().unwrap();

        #[cfg(target_os = "windows")]
        assert_eq!(
            as_pypi_req,
            PixiPypiSpec::Path {
                path: PathBuf::from("C:/path/to/boltons"),
                editable: None,
                extras: vec![]
            }
        );
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            as_pypi_req,
            PixiPypiSpec::Path {
                path: PathBuf::from("/path/to/boltons"),
                editable: None,
                extras: vec![]
            }
        );
    }

    #[test]
    fn test_pep508_git_url() {
        let parsed = pep508_rs::Requirement::from_str(
            "attrs @ git+ssh://git@github.com/python-attrs/attrs.git@main",
        )
        .unwrap();
        assert_eq!(
            PixiPypiSpec::try_from(parsed).unwrap(),
            PixiPypiSpec::Git {
                url: GitSpec {
                    git: Url::parse("ssh://git@github.com/python-attrs/attrs.git").unwrap(),
                    rev: Some(GitReference::Rev("main".to_string())),
                    subdirectory: None
                },

                extras: vec![]
            }
        );

        // With subdirectory
        let parsed = pep508_rs::Requirement::from_str(
            "ribasim@git+https://github.com/Deltares/Ribasim.git#subdirectory=python/ribasim",
        )
        .unwrap();
        assert_eq!(
            PixiPypiSpec::try_from(parsed).unwrap(),
            PixiPypiSpec::Git {
                url: GitSpec {
                    git: Url::parse("https://github.com/Deltares/Ribasim.git").unwrap(),
                    rev: Some(GitReference::DefaultBranch),
                    subdirectory: Some("python/ribasim".to_string()),
                },
                extras: vec![],
            }
        );
    }

    #[test]
    fn test_deserialize_succeeding() {
        const EXAMPLES: &[&str] = &[
            r#"pkg = "==1.2.3""#,
            r#"pkg = { version = "==1.2.3" } "#,
            r#"pkg = "*""#,
            r#"pkg = { path = "foobar" } "#,
            r#"pkg = { path = "~/.cache" } "#,
            r#"pkg = { url = "https://conda.anaconda.org/conda-forge/linux-64/21cmfast-3.3.1-py38h0db86a8_1.conda" }"#,
            r#"pkg = { git = "https://github.com/conda-forge/21cmfast-feedstock" }"#,
            r#"pkg = { git = "https://github.com/conda-forge/21cmfast-feedstock", "branch" = "main" }"#,
            r#"pkg = { git = "ssh://github.com/conda-forge/21cmfast-feedstock", "tag" = "v1.2.3" }"#,
            r#"pkg = { git = "https://github.com/prefix-dev/rattler-build", "rev" = "123456" }"#,
            r#"pkg = { git = "https://github.com/prefix-dev/rattler-build", "subdirectory" = "pyrattler" }"#,
            r#"pkg = { git = "https://github.com/prefix-dev/rattler-build", "extras" = ["test"] }"#,
        ];

        #[derive(Serialize)]
        struct Snapshot {
            input: &'static str,
            result: Value,
        }

        let mut snapshot = Vec::new();
        for input in EXAMPLES {
            let mut toml_value = toml_span::parse(input).unwrap();
            let mut th = TableHelper::new(&mut toml_value).unwrap();
            let req: PixiPypiSpec = th.required("pkg").unwrap();
            let result = serde_json::to_value(&req).unwrap();
            snapshot.push(Snapshot { input, result });
        }

        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_deserialize_failing() {
        const EXAMPLES: &[&str] = &[
            r#"pkg = { ver = "1.2.3" }"#,
            r#"pkg = { path = "foobar", "version" = "==1.2.3" }"#,
            r#"pkg = { version = "//" }"#,
            r#"pkg = { git = "https://github.com/conda-forge/21cmfast-feedstock", branch = "main", tag = "v1" }"#,
            r#"pkg = { git = "https://github.com/conda-forge/21cmfast-feedstock", branch = "main", tag = "v1", "rev" = "123456" }"#,
            r#"pkg = { git = "https://github.com/conda-forge/21cmfast-feedstock", branch = "main", rev = "v1" }"#,
            r#"pkg = { git = "https://github.com/conda-forge/21cmfast-feedstock", tag = "v1", rev = "123456" }"#,
            r#"pkg = { git = "ssh://github.com:conda-forge/21cmfast-feedstock"}"#,
            r#"pkg = { branch = "main", tag = "v1", rev = "123456"  }"#,
            r#"pkg = "/path/style""#,
            r#"pkg = "./path/style""#,
            r#"pkg = "\\path\\style""#,
            r#"pkg = "~/path/style""#,
            r#"pkg = "https://example.com""#,
            r#"pkg = "https://github.com/conda-forge/21cmfast-feedstock""#,
        ];

        struct Snapshot {
            input: &'static str,
            result: Value,
        }

        let mut snapshot = Vec::new();
        for input in EXAMPLES {
            let mut toml_value = toml_span::parse(input).unwrap();
            let mut th = TableHelper::new(&mut toml_value).unwrap();
            let req = th.required::<PixiPypiSpec>("pkg").unwrap_err();

            let result = json!({
                "error": format_parse_error(input, TomlDiagnostic(req))
            });

            snapshot.push(Snapshot { input, result });
        }

        assert_snapshot!(
            snapshot
                .into_iter()
                .map(|Snapshot { input, result }| format!(
                    "input: {input}\nresult: {} ",
                    result
                        .as_object()
                        .unwrap()
                        .get("error")
                        .unwrap()
                        .as_str()
                        .unwrap()
                ))
                .join("\n")
        );
    }
}
