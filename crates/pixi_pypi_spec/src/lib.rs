mod name;
mod pep508;
mod toml;
mod utils;
mod version_or_star;

use std::{
    fmt::{self, Formatter},
    path::PathBuf,
};

use pep440_rs::VersionSpecifiers;
use pep508_rs::{ExtraName, MarkerTree};
use pixi_spec::GitSpec;
use serde::Serialize;
use thiserror::Error;
use url::Url;

pub use name::PypiPackageName;
pub use version_or_star::VersionOrStar;

/// The source of a PyPI package - where/how to obtain it.
///
/// This enum represents the different ways a package can be sourced:
/// - `Registry`: From a package registry with version constraints
/// - `Git`: From a git repository
/// - `Path`: From a local file system path
/// - `Url`: From a direct URL to a package archive
#[derive(Debug, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(untagged, rename_all = "kebab-case")]
pub enum PixiPypiSource {
    /// From a package registry with version constraints.
    Registry {
        version: VersionOrStar,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<Url>,
    },
    /// From a git repository.
    Git {
        #[serde(flatten)]
        git: GitSpec,
    },
    /// From a local file system path (directory or file).
    Path {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        editable: Option<bool>,
    },
    /// From a direct URL to a package archive.
    Url {
        url: Url,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subdirectory: Option<String>,
    },
}

impl PixiPypiSource {
    /// Returns the path if this is a Path source.
    pub fn as_path(&self) -> Option<&PathBuf> {
        match self {
            PixiPypiSource::Path { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Returns the git spec if this is a Git source.
    pub fn as_git(&self) -> Option<&GitSpec> {
        match self {
            PixiPypiSource::Git { git, .. } => Some(git),
            _ => None,
        }
    }

    /// Returns the URL if this is a Url source.
    pub fn as_url(&self) -> Option<&Url> {
        match self {
            PixiPypiSource::Url { url, .. } => Some(url),
            _ => None,
        }
    }

    /// Returns the version if this is a Registry source.
    pub fn as_version(&self) -> Option<&VersionOrStar> {
        match self {
            PixiPypiSource::Registry { version, .. } => Some(version),
            _ => None,
        }
    }

    /// Returns the editability setting if this is a Path source.
    pub fn editable(&self) -> Option<bool> {
        match self {
            PixiPypiSource::Path { editable, .. } => *editable,
            _ => None,
        }
    }

    /// Returns the custom index URL if this is a Registry source.
    pub fn index(&self) -> Option<&Url> {
        match self {
            PixiPypiSource::Registry { index, .. } => index.as_ref(),
            _ => None,
        }
    }

    /// Returns true if this is a source dependency (Git, Path, or Url).
    /// Registry sources are not considered source dependencies.
    pub fn is_source_dependency(&self) -> bool {
        !matches!(self, PixiPypiSource::Registry { .. })
    }
}

impl Default for PixiPypiSource {
    fn default() -> Self {
        PixiPypiSource::Registry {
            version: VersionOrStar::Star,
            index: None,
        }
    }
}

/// Serialize a `pep508_rs::MarkerTree` into a string representation
fn serialize_markertree<S>(value: &MarkerTree, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    // `.expect()` succeeds because we don't serialize when
    // `value.is_true()`, which is the default.
    value.contents().expect("contents were null").serialize(s)
}

/// A complete PyPI dependency specification.
///
/// This is the main type used throughout pixi for PyPI dependencies. It combines
/// the package source (where/how to get it) with optional extras.
///
/// This design follows UV's pattern where common fields like `extras` are at the
/// struct level, and a source is used per spec.
#[derive(Debug, Default, Serialize, Clone, PartialEq, Eq, Hash)]
pub struct PixiPypiSpec {
    /// Optional package extras to install.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extras: Vec<ExtraName>,
    /// The environment markers that decide if/when this package gets installed
    #[serde(
        default,
        // Needed because `pep508_rs::MarkerTree` doesn't implement `serde::Serialize`
        serialize_with = "serialize_markertree",
        skip_serializing_if = "MarkerTree::is_true"
    )]
    pub env_markers: MarkerTree,
    /// The source for this package.
    #[serde(flatten)]
    pub source: PixiPypiSource,
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

impl From<PixiPypiSource> for PixiPypiSpec {
    fn from(source: PixiPypiSource) -> Self {
        PixiPypiSpec {
            extras: Vec::new(),
            source,
            env_markers: MarkerTree::default(),
        }
    }
}

impl PixiPypiSpec {
    /// Creates a new spec with the given source and no extras.
    pub fn new(source: PixiPypiSource) -> Self {
        source.into()
    }

    /// Creates a new spec with the given source and extras.
    pub fn with_extras_and_markers(
        source: PixiPypiSource,
        extras: Vec<ExtraName>,
        env_markers: MarkerTree,
    ) -> Self {
        PixiPypiSpec {
            extras,
            source,
            env_markers,
        }
    }

    /// Returns a reference to the source.
    pub fn source(&self) -> &PixiPypiSource {
        &self.source
    }

    /// Returns a mutable reference to the source.
    pub fn source_mut(&mut self) -> &mut PixiPypiSource {
        &mut self.source
    }

    /// Returns true if this is a source dependency (Git, Path, or Url).
    /// Registry sources are not considered source dependencies.
    pub fn is_source_dependency(&self) -> bool {
        self.source.is_source_dependency()
    }

    /// Returns the path of the requirement if it is a path requirement.
    pub fn as_path(&self) -> Option<&PathBuf> {
        self.source.as_path()
    }

    /// Returns the git spec of the requirement if it is a git requirement.
    pub fn as_git(&self) -> Option<&GitSpec> {
        self.source.as_git()
    }

    /// Returns the url of the requirement if it is a url requirement.
    pub fn as_url(&self) -> Option<&Url> {
        self.source.as_url()
    }

    /// Returns the version of the requirement if it is a version requirement.
    pub fn as_version(&self) -> Option<&VersionOrStar> {
        self.source.as_version()
    }

    /// Define whether the requirement is editable.
    pub fn set_editable(&mut self, editable: bool) {
        match &mut self.source {
            PixiPypiSource::Path { editable: e, .. } => {
                *e = Some(editable);
            }
            _ if editable => {
                tracing::warn!("Ignoring editable flag for non-path requirements.");
            }
            _ => {}
        }
    }

    /// Returns the extras for this spec.
    pub fn extras(&self) -> &[ExtraName] {
        &self.extras
    }

    /// Returns the environment markers for this spec.
    pub fn env_markers(&self) -> &MarkerTree {
        &self.env_markers
    }

    /// Returns the editability setting from the manifest.
    /// Only `Path` specs can be editable. Returns `None` for non-path specs
    /// or if editability is not explicitly specified.
    pub fn editable(&self) -> Option<bool> {
        self.source.editable()
    }

    /// Returns the custom index URL if specified.
    pub fn index(&self) -> Option<&Url> {
        self.source.index()
    }

    /// Updates this spec with a new PEP 508 requirement, preserving pixi-specific
    /// fields (`index`, `extras`) from self.
    ///
    /// This is useful when updating a dependency (e.g., during `pixi upgrade`)
    /// where the version changes but pixi-specific fields like `index` should
    /// be preserved from the original manifest entry.
    pub fn update_requirement(
        &self,
        requirement: &pep508_rs::Requirement,
    ) -> Result<Self, Box<Pep508ToPyPiRequirementError>> {
        let mut updated: PixiPypiSpec = requirement.clone().try_into().map_err(Box::new)?;

        // Preserve index from self if both are Registry sources
        if let (
            PixiPypiSource::Registry {
                index: new_index, ..
            },
            PixiPypiSource::Registry { index, .. },
        ) = (&mut updated.source, &self.source)
        {
            *new_index = index.clone();
        }

        // Preserve extras from self if updated has none
        if updated.extras.is_empty() && !self.extras.is_empty() {
            updated.extras = self.extras.clone();
        }

        updated.env_markers.or(requirement.marker.clone());

        Ok(updated)
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

    // ============================================
    // Tests for the new UV-style struct design
    // ============================================

    #[test]
    fn test_is_source_dependency_for_git() {
        let spec = PixiPypiSpec::new(PixiPypiSource::Git {
            git: GitSpec {
                git: Url::parse("https://github.com/example/repo").unwrap(),
                rev: None,
                subdirectory: None,
            },
        });
        assert!(spec.is_source_dependency());
    }

    #[test]
    fn test_is_source_dependency_for_path() {
        let spec = PixiPypiSpec::new(PixiPypiSource::Path {
            path: PathBuf::from("./local"),
            editable: None,
        });
        assert!(spec.is_source_dependency());
    }

    #[test]
    fn test_is_source_dependency_for_url() {
        let spec = PixiPypiSpec::new(PixiPypiSource::Url {
            url: Url::parse("https://example.com/pkg.whl").unwrap(),
            subdirectory: None,
        });
        assert!(spec.is_source_dependency());
    }

    #[test]
    fn test_is_not_direct_dependency_for_registry() {
        let spec = PixiPypiSpec::new(PixiPypiSource::Registry {
            version: VersionOrStar::Star,
            index: None,
        });
        assert!(!spec.is_source_dependency());
    }

    #[test]
    fn test_extras_accessor() {
        let extra = ExtraName::new("test".to_string()).unwrap();

        // Spec with extras
        let spec = PixiPypiSpec::with_extras_and_markers(
            PixiPypiSource::Git {
                git: GitSpec {
                    git: Url::parse("https://github.com/example/repo").unwrap(),
                    rev: None,
                    subdirectory: None,
                },
            },
            vec![extra.clone()],
            MarkerTree::default(),
        );
        assert_eq!(spec.extras(), std::slice::from_ref(&extra));

        // Spec without extras and markers
        let spec = PixiPypiSpec::new(PixiPypiSource::Registry {
            version: VersionOrStar::Star,
            index: None,
        });
        assert!(spec.extras().is_empty());
    }

    #[test]
    fn test_env_markers_accessor() {
        let markers = MarkerTree::from_str("python_version >= '3.12'").unwrap();
        // Spec with markers
        let spec = PixiPypiSpec::with_extras_and_markers(
            PixiPypiSource::Git {
                git: GitSpec {
                    git: Url::parse("https://github.com/example/repo").unwrap(),
                    rev: None,
                    subdirectory: None,
                },
            },
            vec![],
            markers.clone(),
        );
        assert_eq!(spec.env_markers(), &markers);

        // Spec without extras and markers
        let spec = PixiPypiSpec::new(PixiPypiSource::Registry {
            version: VersionOrStar::Star,
            index: None,
        });
        assert!(spec.env_markers().is_true());
    }

    #[test]
    fn test_source_accessor() {
        let spec = PixiPypiSpec::new(PixiPypiSource::Path {
            path: PathBuf::from("./local"),
            editable: Some(true),
        });

        assert!(matches!(spec.source(), PixiPypiSource::Path { .. }));

        if let PixiPypiSource::Path { editable, .. } = spec.source() {
            assert_eq!(*editable, Some(true));
        } else {
            panic!("Expected Path source");
        }
    }

    #[test]
    fn test_as_version_for_registry() {
        let spec = PixiPypiSpec::new(PixiPypiSource::Registry {
            version: VersionOrStar::Star,
            index: Some(Url::parse("https://pypi.example.com").unwrap()),
        });
        assert!(spec.as_version().is_some());
        assert_eq!(spec.as_version().unwrap(), &VersionOrStar::Star);
    }

    #[test]
    fn test_as_version_returns_none_for_non_registry() {
        let spec = PixiPypiSpec::new(PixiPypiSource::Path {
            path: PathBuf::from("./local"),
            editable: None,
        });
        assert!(spec.as_version().is_none());
    }

    #[test]
    fn test_index_accessor() {
        let index_url = Url::parse("https://pypi.example.com").unwrap();
        let spec = PixiPypiSpec::new(PixiPypiSource::Registry {
            version: VersionOrStar::Star,
            index: Some(index_url.clone()),
        });
        assert_eq!(spec.index(), Some(&index_url));

        // No index
        let spec = PixiPypiSpec::new(PixiPypiSource::Registry {
            version: VersionOrStar::Star,
            index: None,
        });
        assert!(spec.index().is_none());

        // Non-registry source
        let spec = PixiPypiSpec::new(PixiPypiSource::Path {
            path: PathBuf::from("./local"),
            editable: None,
        });
        assert!(spec.index().is_none());
    }

    #[test]
    fn test_from_source_conversion() {
        let source = PixiPypiSource::Path {
            path: PathBuf::from("./local"),
            editable: Some(true),
        };
        let spec: PixiPypiSpec = source.clone().into();
        assert_eq!(spec.source, source);
        assert!(spec.extras.is_empty());
        assert!(spec.env_markers.is_true());
    }

    #[test]
    fn test_default_spec() {
        let spec = PixiPypiSpec::default();
        assert!(spec.extras.is_empty());
        assert!(spec.env_markers.is_true());
        assert!(matches!(
            spec.source,
            PixiPypiSource::Registry {
                version: VersionOrStar::Star,
                index: None
            }
        ));
    }

    #[test]
    fn test_pypi_to_string() {
        let req = pep508_rs::Requirement::from_str("numpy[testing]==1.0.0; os_name == \"posix\"")
            .unwrap();
        let pypi = PixiPypiSpec::try_from(req).unwrap();
        assert_eq!(
            pypi.to_string(),
            "{ version = \"==1.0.0\", extras = [\"testing\"], env-markers = \"os_name == 'posix'\" }"
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
            PixiPypiSpec::new(PixiPypiSource::Git {
                git: GitSpec {
                    git: Url::parse("https://github.com/ecederstrand/exchangelib").unwrap(),
                    rev: Some(GitReference::DefaultBranch),
                    subdirectory: None,
                },
            })
        );

        let pypi: Requirement = "exchangelib @ git+https://github.com/ecederstrand/exchangelib@b283011c6df4a9e034baca9aea19aa8e5a70e3ab".parse().unwrap();
        let as_pypi_req: PixiPypiSpec = pypi.try_into().unwrap();
        assert_eq!(
            as_pypi_req,
            PixiPypiSpec::new(PixiPypiSource::Git {
                git: GitSpec {
                    git: Url::parse("https://github.com/ecederstrand/exchangelib").unwrap(),
                    rev: Some(GitReference::Rev(
                        "b283011c6df4a9e034baca9aea19aa8e5a70e3ab".to_string()
                    )),
                    subdirectory: None,
                },
            })
        );

        let pypi: Requirement = "boltons @ https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl".parse().unwrap();
        let as_pypi_req: PixiPypiSpec = pypi.try_into().unwrap();
        assert_eq!(as_pypi_req, PixiPypiSpec::new(PixiPypiSource::Url { url: Url::parse("https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl").unwrap(), subdirectory: None }));

        let pypi: Requirement = "boltons[nichita] @ https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl".parse().unwrap();
        let as_pypi_req: PixiPypiSpec = pypi.try_into().unwrap();
        assert_eq!(as_pypi_req, PixiPypiSpec::with_extras_and_markers(PixiPypiSource::Url { url: Url::parse("https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl").unwrap(), subdirectory: None }, vec![ExtraName::new("nichita".to_string()).unwrap()], MarkerTree::default()));

        let pypi: Requirement = "potato[habbasi]; sys_platform == 'linux'".parse().unwrap();
        let as_pypi_req: PixiPypiSpec = pypi.try_into().unwrap();
        assert_snapshot!(as_pypi_req);

        #[cfg(target_os = "windows")]
        let pypi: Requirement = "boltons @ file:///C:/path/to/boltons".parse().unwrap();
        #[cfg(not(target_os = "windows"))]
        let pypi: Requirement = "boltons @ file:///path/to/boltons".parse().unwrap();

        let as_pypi_req: PixiPypiSpec = pypi.try_into().unwrap();

        #[cfg(target_os = "windows")]
        assert_eq!(
            as_pypi_req,
            PixiPypiSpec::new(PixiPypiSource::Path {
                path: PathBuf::from("C:/path/to/boltons"),
                editable: None,
            })
        );
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            as_pypi_req,
            PixiPypiSpec::new(PixiPypiSource::Path {
                path: PathBuf::from("/path/to/boltons"),
                editable: None,
            })
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
            PixiPypiSpec::new(PixiPypiSource::Git {
                git: GitSpec {
                    git: Url::parse("ssh://git@github.com/python-attrs/attrs.git").unwrap(),
                    rev: Some(GitReference::Rev("main".to_string())),
                    subdirectory: None
                },
            })
        );

        // With subdirectory
        let parsed = pep508_rs::Requirement::from_str(
            "ribasim@git+https://github.com/Deltares/Ribasim.git#subdirectory=python/ribasim",
        )
        .unwrap();
        assert_eq!(
            PixiPypiSpec::try_from(parsed).unwrap(),
            PixiPypiSpec::new(PixiPypiSource::Git {
                git: GitSpec {
                    git: Url::parse("https://github.com/Deltares/Ribasim.git").unwrap(),
                    rev: Some(GitReference::DefaultBranch),
                    subdirectory: Some("python/ribasim".to_string()),
                },
            })
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
            r#"pkg = { version = "*", "env-markers" = "sys_platform == 'win32'" }"#,
            r#"pkg = { git = "https://github.com/prefix-dev/rattler-build", "extras" = ["test"], "env-markers" = "sys_platform == 'linux'" }"#,
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
            r#"pkg = { version = "*", "env-markers" = "potato == 'potato'" }"#,
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

        insta::assert_snapshot!(
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
