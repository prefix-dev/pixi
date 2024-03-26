use pep440_rs::VersionSpecifiers;
use pep508_rs::VerbatimUrl;
use serde::Serializer;
use serde::{de::Error, Deserialize, Deserializer, Serialize};
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::{fmt, fmt::Formatter, str::FromStr};
use thiserror::Error;
use toml_edit::Item;
use url::Url;

use uv_normalize::{ExtraName, InvalidNameError, PackageName};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
/// A package name for PyPI that also stores the source version of the name.
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

impl FromStr for PyPiPackageName {
    type Err = InvalidNameError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            source: name.to_string(),
            normalized: uv_normalize::PackageName::from_str(name)?,
        })
    }
}

impl PyPiPackageName {
    pub fn from_normalized(normalized: PackageName) -> Self {
        Self {
            source: normalized.to_string(),
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

impl Serialize for VersionOrStar {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for VersionOrStar {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        VersionOrStar::from_str(&s).map_err(serde::de::Error::custom)
    }
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
        version: VersionOrStar,
        index: Option<String>,
        #[serde(default)]
        extras: Vec<ExtraName>,
    },
    RawVersion(VersionOrStar),
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
                todo!("git")
            }
            PyPiRequirement::Path {
                path: _,
                editable: _,
                extras: _,
            } => {
                todo!("path")
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
        } else if !req.extras.is_empty() {
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

/// Create a url that uv can use to install a version
fn create_uv_url(
    url: &Url,
    rev: Option<&str>,
    subdir: Option<&str>,
) -> Result<Url, url::ParseError> {
    // Create the url.
    let url = format!("git+{url}");
    // Add the tag or rev if it exists.
    let url = rev
        .as_ref()
        .map_or_else(|| url.clone(), |tag_or_rev| format!("{url}@{tag_or_rev}"));

    // Add the subdirectory if it exists.
    let url = subdir.as_ref().map_or_else(
        || url.clone(),
        |subdir| format!("{url}#subdirectory={subdir}"),
    );
    url.parse()
}

#[derive(Error, Debug)]
pub enum AsPep508Error {
    #[error("error while canonicalizing {path}")]
    CanonicalizeError {
        source: std::io::Error,
        path: PathBuf,
    },
    #[error("parsing url {url}")]
    UrlParseError {
        source: url::ParseError,
        url: String,
    },
    #[error("using an editable flag for a path that is not a directory: {path}")]
    EditableIsNotDir { path: PathBuf },
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum RequirementOrEditable {
    Editable(PackageName, requirements_txt::EditableRequirement),
    Pep508Requirement(pep508_rs::Requirement),
}

impl Display for RequirementOrEditable {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            RequirementOrEditable::Editable(name, req) => {
                write!(f, "{} = {:?}", name, req)
            }
            RequirementOrEditable::Pep508Requirement(req) => {
                write!(f, "{}", req)
            }
        }
    }
}

impl RequirementOrEditable {
    /// Returns the name of the package that this requirement is for.
    pub fn name(&self) -> &PackageName {
        match self {
            RequirementOrEditable::Editable(name, _) => name,
            RequirementOrEditable::Pep508Requirement(req) => &req.name,
        }
    }

    /// Returns any extras that this requirement has.
    pub fn extras(&self) -> &[ExtraName] {
        match self {
            RequirementOrEditable::Editable(_, req) => &req.extras,
            RequirementOrEditable::Pep508Requirement(req) => &req.extras,
        }
    }

    /// Returns an editable requirement if it is an editable requirement.
    pub fn into_editable(self) -> Option<requirements_txt::EditableRequirement> {
        match self {
            RequirementOrEditable::Editable(_, editable) => Some(editable),
            _ => None,
        }
    }

    /// Returns a pep508 requirement if it is a pep508 requirement.
    pub fn into_requirement(self) -> Option<pep508_rs::Requirement> {
        match self {
            RequirementOrEditable::Pep508Requirement(e) => Some(e),
            _ => None,
        }
    }

    /// Returns an editable requirement if it is an editable requirement.
    pub fn as_editable(&self) -> Option<&requirements_txt::EditableRequirement> {
        match self {
            RequirementOrEditable::Editable(_name, editable) => Some(editable),
            _ => None,
        }
    }

    /// Returns a pep508 requirement if it is a pep508 requirement.
    pub fn as_requirement(&self) -> Option<&pep508_rs::Requirement> {
        match self {
            RequirementOrEditable::Pep508Requirement(e) => Some(e),
            _ => None,
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
    pub fn as_pep508(
        &self,
        name: &PackageName,
        project_root: &Path,
    ) -> Result<RequirementOrEditable, AsPep508Error> {
        let version_or_url = match self {
            PyPiRequirement::Version {
                version,
                index: _,
                extras: _,
            } => version.clone().into(),
            PyPiRequirement::Path {
                path,
                editable,
                extras,
            } => {
                let joined = project_root.join(path);
                let canonicalized =
                    dunce::canonicalize(&joined).map_err(|e| AsPep508Error::CanonicalizeError {
                        source: e,
                        path: joined.clone(),
                    })?;
                let given = path
                    .to_str()
                    .map(|s| s.to_owned())
                    .unwrap_or_else(String::new);
                let verbatim = VerbatimUrl::from_path(canonicalized.clone()).with_given(given);

                if *editable == Some(true) {
                    if !canonicalized.is_dir() {
                        return Err(AsPep508Error::EditableIsNotDir { path: path.clone() });
                    }

                    return Ok(RequirementOrEditable::Editable(
                        name.clone(),
                        requirements_txt::EditableRequirement {
                            url: verbatim,
                            extras: extras.clone(),
                            path: canonicalized,
                        },
                    ));
                }

                Some(pep508_rs::VersionOrUrl::Url(verbatim))
            }
            PyPiRequirement::Git {
                git,
                branch,
                tag,
                rev,
                subdirectory: subdir,
                extras: _,
            } => {
                if branch.is_some() && tag.is_some() {
                    tracing::warn!("branch/tag are not supported *yet*, will use the `main`/`master` branch, please specify a revision using `rev` = `sha`");
                }
                let uv_url =
                    create_uv_url(git, rev.as_deref(), subdir.as_deref()).map_err(|e| {
                        AsPep508Error::UrlParseError {
                            source: e,
                            url: git.to_string(),
                        }
                    })?;
                Some(pep508_rs::VersionOrUrl::Url(VerbatimUrl::from_url(uv_url)))
            }
            PyPiRequirement::Url { url, extras: _ } => Some(pep508_rs::VersionOrUrl::Url(
                VerbatimUrl::from_url(url.clone()),
            )),
            PyPiRequirement::RawVersion(version) => version.clone().into(),
        };

        Ok(RequirementOrEditable::Pep508Requirement(
            pep508_rs::Requirement {
                name: name.clone(),
                extras: self.extras().to_vec(),
                version_or_url,
                marker: None,
            },
        ))
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
        let pypi: Requirement = "numpy".parse().unwrap();
        let as_pypi_req: PyPiRequirement = pypi.into();
        // convert to toml and snapshot
        assert_snapshot!(as_pypi_req.to_string());

        let pypi: Requirement = "numpy[test,extrastuff]".parse().unwrap();
        let as_pypi_req: PyPiRequirement = pypi.into();
        // convert to toml and snapshot
        assert_snapshot!(as_pypi_req.to_string());
    }
}
