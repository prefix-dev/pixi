use std::{
    borrow::Borrow,
    fmt,
    fmt::Formatter,
    path::{Path, PathBuf},
    str::FromStr,
};

use pep440_rs::VersionSpecifiers;
use pep508_rs::VerbatimUrl;
use pypi_types::RequirementSource;
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use url::Url;
use uv_git::{GitReference, GitSha};
use uv_normalize::{ExtraName, InvalidNameError, PackageName};

use crate::util::extract_directory_from_url;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
/// A package name for PyPI that also stores the source version of the name.
pub struct PyPiPackageName {
    source: String,
    normalized: PackageName,
}

impl Borrow<PackageName> for PyPiPackageName {
    fn borrow(&self) -> &PackageName {
        &self.normalized
    }
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

impl From<VersionOrStar> for VersionSpecifiers {
    fn from(value: VersionOrStar) -> Self {
        match value {
            VersionOrStar::Version(v) => v,
            VersionOrStar::Star => VersionSpecifiers::empty(),
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

#[derive(Debug, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(untagged, rename_all = "snake_case", deny_unknown_fields)]
pub enum GitRev {
    Short(String),
    Full(String),
}

impl GitRev {
    fn as_full(&self) -> Option<&str> {
        match self {
            GitRev::Full(full) => Some(full.as_str()),
            GitRev::Short(_) => None,
        }
    }

    fn to_git_reference(&self) -> GitReference {
        match self {
            GitRev::Full(rev) => GitReference::FullCommit(rev.clone()),
            GitRev::Short(rev) => GitReference::BranchOrTagOrCommit(rev.clone()),
        }
    }
}

impl From<&str> for GitRev {
    fn from(s: &str) -> Self {
        if s.len() == 40 {
            GitRev::Full(s.to_string())
        } else {
            GitRev::Short(s.to_string())
        }
    }
}

impl ToString for GitRev {
    fn to_string(&self) -> String {
        match self {
            GitRev::Short(s) => s.clone(),
            GitRev::Full(s) => s.clone(),
        }
    }
}

impl<'de> Deserialize<'de> for GitRev {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        if s.len() == 40 {
            Ok(GitRev::Full(s))
        } else {
            Ok(GitRev::Short(s))
        }
    }
}

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

/// Implement from [`pep508_rs::Requirement`] to make the conversion easier.
impl From<pep508_rs::Requirement> for PyPiRequirement {
    fn from(req: pep508_rs::Requirement) -> Self {
        if let Some(version_or_url) = req.version_or_url {
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
                            let url = Url::parse(url)
                                .expect("expect proper url as it is previously parsed");
                            PyPiRequirement::Git {
                                git: url,
                                branch: None,
                                tag: None,
                                rev: Some(GitRev::from(version)),
                                subdirectory: None,
                                extras: req.extras,
                            }
                        } else {
                            let url = Url::parse(stripped_url)
                                .expect("expect proper url as it is previously parsed");
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
                            let file = url
                                .to_file_path()
                                .expect("could not convert to file url to path");
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
        }
    }
}

/// Create a url that uv can use to install a version
fn create_uv_url(
    url: &Url,
    rev: Option<&GitRev>,
    subdir: Option<&str>,
) -> Result<Url, url::ParseError> {
    // Create the url.
    let url = format!("git+{url}");
    // Add the tag or rev if it exists.
    let url = rev.as_ref().map_or_else(
        || url.clone(),
        |tag_or_rev| format!("{url}@{}", tag_or_rev.to_string()),
    );

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
    #[error("error while canonicalizing {0}")]
    VerabatimUrlError(#[from] pep508_rs::VerbatimUrlError),
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

    /// Convert into a `pypi_types::Requirement`, which is an uv extended requirement type
    pub fn as_uv_req(
        &self,
        name: &PackageName,
        project_root: &Path,
    ) -> Result<pypi_types::Requirement, AsPep508Error> {
        let source = match self {
            PyPiRequirement::Version { version, .. } => {
                // TODO: implement index later
                RequirementSource::Registry {
                    specifier: version.clone().into(),
                    index: None,
                }
            }
            PyPiRequirement::Git {
                git,
                rev,
                tag,
                subdirectory,
                branch,
                ..
            } => RequirementSource::Git {
                repository: git.clone(),
                precise: rev
                    .as_ref()
                    .map(|s| s.as_full())
                    .and_then(|s| s.map(GitSha::from_str))
                    .transpose()
                    .expect("could not parse sha"),
                reference: tag
                    .as_ref()
                    .map(|tag| GitReference::Tag(tag.clone()))
                    .or(branch
                        .as_ref()
                        .map(|branch| GitReference::Branch(branch.to_string())))
                    .or(rev.as_ref().map(|rev| rev.to_git_reference()))
                    .unwrap_or(GitReference::DefaultBranch),
                subdirectory: subdirectory.as_ref().and_then(|s| s.parse().ok()),
                url: VerbatimUrl::from_url(
                    create_uv_url(git, rev.as_ref(), subdirectory.as_deref()).map_err(|e| {
                        AsPep508Error::UrlParseError {
                            source: e,
                            url: git.to_string(),
                        }
                    })?,
                ),
            },
            PyPiRequirement::Path {
                path,
                editable,
                extras: _,
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
                let verbatim = VerbatimUrl::from_path(canonicalized.clone())?.with_given(given);

                RequirementSource::Path {
                    install_path: canonicalized,
                    lock_path: path.clone(),
                    editable: editable.unwrap_or_default(),
                    url: verbatim,
                }
            }
            PyPiRequirement::Url {
                url, subdirectory, ..
            } => {
                RequirementSource::Url {
                    // TODO: fill these later
                    subdirectory: subdirectory.as_ref().map(|sub| PathBuf::from(sub.as_str())),
                    location: url.clone(),
                    url: VerbatimUrl::from_url(url.clone()),
                }
            }
            PyPiRequirement::RawVersion(version) => RequirementSource::Registry {
                specifier: version.clone().into(),
                index: None,
            },
        };

        Ok(pypi_types::Requirement {
            name: name.clone(),
            extras: self.extras().to_vec(),
            marker: None,
            source,
            origin: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use indexmap::IndexMap;
    use insta::assert_snapshot;
    use pep508_rs::Requirement;

    use super::*;

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
                    foo = { version=">=3.12", extras = ["bar"]}
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
        let as_pypi_req: PyPiRequirement = pypi.into();
        // convert to toml and snapshot
        assert_snapshot!(as_pypi_req.to_string());

        let pypi: Requirement = "numpy[test,extrastuff]".parse().unwrap();
        let as_pypi_req: PyPiRequirement = pypi.into();
        // convert to toml and snapshot
        assert_snapshot!(as_pypi_req.to_string());

        let pypi: Requirement = "exchangelib @ git+https://github.com/ecederstrand/exchangelib"
            .parse()
            .unwrap();
        let as_pypi_req: PyPiRequirement = pypi.into();
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
        let as_pypi_req: PyPiRequirement = pypi.into();
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
        let as_pypi_req: PyPiRequirement = pypi.into();
        assert_eq!(as_pypi_req, PyPiRequirement::Url{url: Url::parse("https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl").unwrap(), extras: vec![], subdirectory: None });

        let pypi: Requirement = "boltons[nichita] @ https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl".parse().unwrap();
        let as_pypi_req: PyPiRequirement = pypi.into();
        assert_eq!(as_pypi_req, PyPiRequirement::Url{url: Url::parse("https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl").unwrap(), extras: vec![ExtraName::new("nichita".to_string()).unwrap()], subdirectory: None });

        #[cfg(target_os = "windows")]
        let pypi: Requirement = "boltons @ file:///C:/path/to/boltons".parse().unwrap();
        #[cfg(not(target_os = "windows"))]
        let pypi: Requirement = "boltons @ file:///path/to/boltons".parse().unwrap();

        let as_pypi_req: PyPiRequirement = pypi.into();

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
