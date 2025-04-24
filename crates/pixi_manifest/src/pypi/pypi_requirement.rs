use std::{
    fmt,
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
};

use itertools::Itertools;
use pep440_rs::VersionSpecifiers;
use pep508_rs::ExtraName;
use pixi_spec::{GitReference, GitSpec};
use pixi_toml::{TomlFromStr, TomlWith};
use serde::Serialize;
use thiserror::Error;
use toml_span::{DeserError, Value, de_helpers::TableHelper};
use url::Url;

use pixi_git::GitUrl;

use super::VersionOrStar;
use crate::utils::extract_directory_from_url;

#[derive(Debug, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(untagged, rename_all = "kebab-case", deny_unknown_fields)]
pub enum PyPiRequirement {
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

/// Returns a more helpful message when a version requirement is used
/// incorrectly.
fn version_requirement_error<T: Into<String>>(input: T) -> Option<impl Display> {
    let input = input.into();
    if input.starts_with('/')
        || input.starts_with('.')
        || input.starts_with('\\')
        || input.starts_with("~/")
    {
        return Some(format!(
            "it seems you're trying to add a path dependency, please specify as a table with a `path` key: '{{ path = \"{input}\" }}'"
        ));
    }

    if input.contains("git") {
        return Some(format!(
            "it seems you're trying to add a git dependency, please specify as a table with a `git` key: '{{ git = \"{input}\" }}'"
        ));
    }

    if input.contains("://") {
        return Some(format!(
            "it seems you're trying to add a url dependency, please specify as a table with a `url` key: '{{ url = \"{input}\" }}'"
        ));
    }

    None
}

struct RawPyPiRequirement {
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    pub version: Option<VersionOrStar>,

    extras: Vec<ExtraName>,

    // Path Only
    pub path: Option<PathBuf>,
    pub editable: Option<bool>,

    // Git only
    pub git: Option<Url>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub rev: Option<String>,

    // Url only
    pub url: Option<Url>,

    // Git and Url only
    pub subdirectory: Option<String>,

    // Pinned index
    pub index: Option<Url>,
}

impl Default for PyPiRequirement {
    fn default() -> Self {
        PyPiRequirement::RawVersion(VersionOrStar::Star)
    }
}

#[derive(Error, Debug)]
pub enum SpecConversion {
    #[error("`branch`, `rev`, and `tag` are only valid when `git` is specified")]
    MissingGit,
    #[error("Only one of `branch` or `tag` or `rev` can be specified")]
    MultipleGitSpecifiers,
    #[error("`version` cannot be used with {non_detailed_keys}")]
    VersionWithNonDetailedKeys { non_detailed_keys: String },
    #[error("Exactly one of `url`, `path`, `git`, or `version` must be specified")]
    MultipleVersionSpecifiers,
}

impl RawPyPiRequirement {
    fn into_pypi_requirement(self) -> Result<PyPiRequirement, SpecConversion> {
        if self.git.is_none() && (self.branch.is_some() || self.rev.is_some() || self.tag.is_some())
        {
            return Err(SpecConversion::MissingGit);
        }

        // Only one of the git version specifiers can be used.
        if self.branch.is_some() && self.tag.is_some()
            || self.branch.is_some() && self.rev.is_some()
            || self.tag.is_some() && self.rev.is_some()
        {
            return Err(SpecConversion::MultipleGitSpecifiers);
        }

        let is_git = self.git.is_some();
        let is_path = self.path.is_some();
        let is_url = self.url.is_some();

        let git_key = is_git.then_some("`git`");
        let path_key = is_path.then_some("`path`");
        let url_key = is_url.then_some("`url`");
        let non_detailed_keys = [git_key, path_key, url_key]
            .into_iter()
            .flatten()
            .format(", ")
            .to_string();

        if !non_detailed_keys.is_empty() && self.version.is_some() {
            return Err(SpecConversion::VersionWithNonDetailedKeys { non_detailed_keys });
        }

        let req = match (self.url, self.path, self.git, self.extras, self.index) {
            (Some(url), None, None, extras, None) => PyPiRequirement::Url {
                url,
                extras,
                subdirectory: self.subdirectory,
            },
            (None, Some(path), None, extras, None) => PyPiRequirement::Path {
                path,
                editable: self.editable,
                extras,
            },
            (None, None, Some(git), extras, None) => {
                let rev = match (self.branch, self.rev, self.tag) {
                    (Some(branch), None, None) => Some(GitReference::Branch(branch)),
                    (None, Some(rev), None) => Some(GitReference::Rev(rev)),
                    (None, None, Some(tag)) => Some(GitReference::Tag(tag)),
                    (None, None, None) => None,
                    _ => {
                        return Err(SpecConversion::MultipleGitSpecifiers);
                    }
                };
                PyPiRequirement::Git {
                    url: GitSpec {
                        git,
                        rev,
                        subdirectory: self.subdirectory,
                    },
                    extras,
                }
            }
            (None, None, None, extras, index) => PyPiRequirement::Version {
                version: self.version.unwrap_or(VersionOrStar::Star),
                extras,
                index,
            },
            (_, _, _, extras, index) if !extras.is_empty() => PyPiRequirement::Version {
                version: self.version.unwrap_or(VersionOrStar::Star),
                extras,
                index,
            },
            _ => {
                return Err(SpecConversion::MultipleVersionSpecifiers);
            }
        };

        Ok(req)
    }
}

impl<'de> toml_span::Deserialize<'de> for RawPyPiRequirement {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let version = th.optional("version");
        let extras = th
            .optional::<TomlWith<_, Vec<TomlFromStr<_>>>>("extras")
            .map(TomlWith::into_inner)
            .unwrap_or_default();

        let path = th
            .optional::<TomlFromStr<_>>("path")
            .map(TomlFromStr::into_inner);
        let editable = th.optional("editable");

        let git = th
            .optional::<TomlFromStr<_>>("git")
            .map(TomlFromStr::into_inner);
        let branch = th.optional("branch");
        let tag = th.optional("tag");
        let rev = th
            .optional::<TomlFromStr<_>>("rev")
            .map(TomlFromStr::into_inner);

        let url = th
            .optional::<TomlFromStr<_>>("url")
            .map(TomlFromStr::into_inner);

        let subdirectory = th.optional("subdirectory");

        let index = th
            .optional::<TomlFromStr<_>>("index")
            .map(TomlFromStr::into_inner);

        th.finalize(None)?;

        Ok(RawPyPiRequirement {
            version,
            extras,
            path,
            editable,
            git,
            branch,
            tag,
            rev,
            url,
            subdirectory,
            index,
        })
    }
}

impl<'de> toml_span::Deserialize<'de> for PyPiRequirement {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        if let Some(str) = value.as_str() {
            return Ok(PyPiRequirement::RawVersion(
                VersionOrStar::from_str(str).map_err(|e| toml_span::Error {
                    kind: toml_span::ErrorKind::Custom(
                        version_requirement_error(str)
                            .map_or(e.to_string().into(), |e| e.to_string().into()),
                    ),
                    span: value.span,
                    line_info: None,
                })?,
            ));
        }

        <RawPyPiRequirement as toml_span::Deserialize>::deserialize(value)?
            .into_pypi_requirement()
            .map_err(|e| {
                toml_span::Error {
                    kind: toml_span::ErrorKind::Custom(e.to_string().into()),
                    span: value.span,
                    line_info: None,
                }
                .into()
            })
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

        fn insert_index(table: &mut toml_edit::InlineTable, index: &Option<Url>) {
            if let Some(index) = index {
                table.insert(
                    "index",
                    toml_edit::Value::String(toml_edit::Formatted::new(index.to_string())),
                );
            }
        }

        match &val {
            PyPiRequirement::Version {
                version,
                extras,
                index,
            } if extras.is_empty() && index.is_none() => {
                toml_edit::Value::from(version.to_string())
            }
            PyPiRequirement::Version {
                version,
                extras,
                index,
            } => {
                let mut table = toml_edit::Table::new().into_inline_table();
                table.insert(
                    "version",
                    toml_edit::Value::String(toml_edit::Formatted::new(version.to_string())),
                );
                insert_extras(&mut table, extras);
                insert_index(&mut table, index);
                toml_edit::Value::InlineTable(table.to_owned())
            }
            PyPiRequirement::Git {
                url:
                    GitSpec {
                        git,
                        rev,
                        subdirectory,
                    },
                extras,
            } => {
                let mut table = toml_edit::Table::new().into_inline_table();
                table.insert(
                    "git",
                    toml_edit::Value::String(toml_edit::Formatted::new(git.to_string())),
                );

                if let Some(rev) = rev {
                    match rev {
                        GitReference::Branch(branch) => {
                            table.insert(
                                "branch",
                                toml_edit::Value::String(toml_edit::Formatted::new(branch.clone())),
                            );
                        }
                        GitReference::Tag(tag) => {
                            table.insert(
                                "tag",
                                toml_edit::Value::String(toml_edit::Formatted::new(tag.clone())),
                            );
                        }
                        GitReference::Rev(rev) => {
                            table.insert(
                                "rev",
                                toml_edit::Value::String(toml_edit::Formatted::new(rev.clone())),
                            );
                        }
                        GitReference::DefaultBranch => {}
                    }
                };

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

/// Implement from [`pep508_rs::Requirement`] to make the conversion easier.
impl TryFrom<pep508_rs::Requirement> for PyPiRequirement {
    type Error = Pep508ToPyPiRequirementError;
    fn try_from(req: pep508_rs::Requirement) -> Result<Self, Self::Error> {
        let converted = if let Some(version_or_url) = req.version_or_url {
            match version_or_url {
                pep508_rs::VersionOrUrl::VersionSpecifier(v) => PyPiRequirement::Version {
                    version: v.into(),
                    extras: req.extras,
                    index: None,
                },
                pep508_rs::VersionOrUrl::Url(u) => {
                    let url = u.to_url();
                    if let Some((prefix, ..)) = url.scheme().split_once('+') {
                        match prefix {
                            "git" => {
                                let subdirectory = extract_directory_from_url(&url);
                                let git_url = GitUrl::try_from(url).unwrap();
                                let git_spec = GitSpec {
                                    git: git_url.repository().clone(),
                                    rev: Some(git_url.reference().clone().into()),
                                    subdirectory,
                                };

                                Self::Git {
                                    url: git_spec,
                                    extras: req.extras,
                                }
                            }
                            "bzr" => {
                                return Err(Pep508ToPyPiRequirementError::UnsupportedUrlPrefix {
                                    prefix: prefix.to_string(),
                                    url: u.to_url(),
                                    message: "Bazaar is not supported",
                                });
                            }
                            "hg" => {
                                return Err(Pep508ToPyPiRequirementError::UnsupportedUrlPrefix {
                                    prefix: prefix.to_string(),
                                    url: u.to_url(),
                                    message: "Bazaar is not supported",
                                });
                            }
                            "svn" => {
                                return Err(Pep508ToPyPiRequirementError::UnsupportedUrlPrefix {
                                    prefix: prefix.to_string(),
                                    url: u.to_url(),
                                    message: "Bazaar is not supported",
                                });
                            }
                            _ => {
                                return Err(Pep508ToPyPiRequirementError::UnsupportedUrlPrefix {
                                    prefix: prefix.to_string(),
                                    url: u.to_url(),
                                    message: "Unknown scheme",
                                });
                            }
                        }
                    } else if Path::new(url.path())
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("git"))
                    {
                        let git_url = GitUrl::try_from(url).unwrap();
                        let subdirectory = extract_directory_from_url(git_url.repository());
                        let git_spec = GitSpec {
                            git: git_url.repository().clone(),
                            rev: Some(git_url.reference().clone().into()),
                            subdirectory,
                        };
                        Self::Git {
                            url: git_spec,
                            extras: req.extras,
                        }
                    } else if url.scheme().eq_ignore_ascii_case("file") {
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
        } else if !req.extras.is_empty() {
            PyPiRequirement::Version {
                version: VersionOrStar::Star,
                extras: req.extras,
                index: None,
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

    /// Returns the path of the requirement if it is a path requirement.
    pub fn as_path(&self) -> Option<&PathBuf> {
        match self {
            PyPiRequirement::Path { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Returns the git spec of the requirement if it is a git requirement.
    pub fn as_git(&self) -> Option<&GitSpec> {
        match self {
            PyPiRequirement::Git { url, .. } => Some(url),
            _ => None,
        }
    }

    /// Returns the url of the requirement if it is a url requirement.
    pub fn as_url(&self) -> Option<&Url> {
        match self {
            PyPiRequirement::Url { url, .. } => Some(url),
            _ => None,
        }
    }

    /// Returns the version of the requirement if it is a version requirement.
    pub fn as_version(&self) -> Option<&VersionOrStar> {
        match self {
            PyPiRequirement::Version { version, .. } | PyPiRequirement::RawVersion(version) => {
                Some(version)
            }
            _ => None,
        }
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

    use insta::assert_snapshot;
    use pep508_rs::Requirement;
    use pixi_toml::TomlIndexMap;
    use serde_json::{Value, json};
    use toml_span::{Deserialize, value::ValueInner};

    use super::*;
    use crate::{TomlError, toml::FromTomlStr, utils::test_utils::format_parse_error};

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
        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"foo = ">=3.12""#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().0,
            &pep508_rs::PackageName::from_str("foo").unwrap()
        );
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::RawVersion(">=3.12".parse().unwrap())
        );

        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"foo = "==3.12.0""#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::RawVersion("==3.12.0".parse().unwrap())
        );

        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"foo = "~=2.1.3""#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::RawVersion("~=2.1.3".parse().unwrap())
        );

        let requirement =
            TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(r#"foo = "*""#)
                .unwrap()
                .into_inner();
        assert_eq!(requirement.first().unwrap().1, &PyPiRequirement::default());
    }

    #[test]
    fn test_extended() {
        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"
                    foo = { version=">=3.12", extras = ["bar"]}
                    "#,
        )
        .unwrap()
        .into_inner();

        assert_eq!(
            requirement.first().unwrap().0,
            &pep508_rs::PackageName::from_str("foo").unwrap()
        );
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::Version {
                version: ">=3.12".parse().unwrap(),
                extras: vec![ExtraName::from_str("bar").unwrap()],
                index: None,
            }
        );

        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"bar = { version=">=3.12,<3.13.0", extras = ["bar", "foo"] }"#,
        )
        .unwrap()
        .into_inner();
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
                index: None,
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_requirement_from_map() {
        let pypi_requirement = PyPiRequirement::from_toml_str(
            r#"
        version = "==1.2.3"
        extras = ["feature1", "feature2"]
        "#,
        )
        .unwrap();

        assert_eq!(
            pypi_requirement,
            PyPiRequirement::Version {
                version: "==1.2.3".parse().unwrap(),
                extras: vec![
                    ExtraName::from_str("feature1").unwrap(),
                    ExtraName::from_str("feature2").unwrap()
                ],
                index: None,
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_requirement_from_str() {
        let pypi_requirement = PyPiRequirement::deserialize(&mut toml_span::Value::new(
            ValueInner::String(r#"==1.2.3"#.into()),
        ))
        .unwrap();
        assert_eq!(
            pypi_requirement,
            PyPiRequirement::RawVersion("==1.2.3".parse().unwrap())
        );
    }

    #[test]
    fn test_deserialize_pypi_requirement_from_str_with_star() {
        let pypi_requirement = PyPiRequirement::deserialize(&mut toml_span::Value::new(
            ValueInner::String("*".into()),
        ))
        .unwrap();
        assert_eq!(pypi_requirement, PyPiRequirement::default());
    }

    #[test]
    fn test_deserialize_pypi_from_path() {
        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"foo = { path = "../numpy-test" }"#,
        )
        .unwrap()
        .into_inner();
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
        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"foo = { path = "../numpy-test", editable = true }"#,
        )
        .unwrap()
        .into_inner();
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
        let input = r#"foo = { borked = "bork"}"#;
        assert_snapshot!(format_parse_error(input, TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(input).unwrap_err()), @r###"
         × Unexpected keys, expected only 'version', 'extras', 'path', 'editable', 'git', 'branch', 'tag', 'rev', 'url', 'subdirectory', 'index'
          ╭─[pixi.toml:1:9]
        1 │ foo = { borked = "bork"}
          ·         ───┬──
          ·            ╰── 'borked' was not expected here
          ╰────
        "###);
    }

    #[test]
    fn test_deserialize_pypi_from_url() {
        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"foo = { url = "https://test.url.com"}"#,
        )
        .unwrap()
        .into_inner();

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
        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"foo = { git = "https://test.url.git" }"#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::Git {
                url: GitSpec {
                    git: Url::parse("https://test.url.git").unwrap(),
                    rev: None,
                    subdirectory: None,
                },
                extras: vec![],
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_from_git_branch() {
        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"foo = { git = "https://test.url.git", branch = "main" }"#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::Git {
                url: GitSpec {
                    git: Url::parse("https://test.url.git").unwrap(),
                    rev: Some(GitReference::Branch("main".to_string())),
                    subdirectory: None,
                },
                extras: vec![],
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_from_git_tag() {
        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"foo = { git = "https://test.url.git", tag = "v.1.2.3" }"#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::Git {
                url: GitSpec {
                    git: Url::parse("https://test.url.git").unwrap(),
                    rev: Some(GitReference::Tag("v.1.2.3".to_string())),
                    subdirectory: None,
                },
                extras: vec![],
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_from_flask() {
        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"flask = { git = "https://github.com/pallets/flask.git", tag = "3.0.0"}"#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::Git {
                url: GitSpec {
                    git: Url::parse("https://github.com/pallets/flask.git").unwrap(),
                    rev: Some(GitReference::Tag("3.0.0".to_string())),
                    subdirectory: None,
                },
                extras: vec![],
            },
        );
    }

    #[test]
    fn test_deserialize_pypi_from_git_rev() {
        let requirement = TomlIndexMap::<pep508_rs::PackageName, PyPiRequirement>::from_toml_str(
            r#"foo = { git = "https://test.url.git", rev = "123456" }"#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement::Git {
                url: GitSpec {
                    git: Url::parse("https://test.url.git").unwrap(),
                    rev: Some(GitReference::Rev("123456".to_string())),
                    subdirectory: None,
                },
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
                url: GitSpec {
                    git: Url::parse("https://github.com/ecederstrand/exchangelib").unwrap(),
                    rev: Some(GitReference::DefaultBranch),
                    subdirectory: None,
                },
                extras: vec![]
            }
        );

        let pypi: Requirement = "exchangelib @ git+https://github.com/ecederstrand/exchangelib@b283011c6df4a9e034baca9aea19aa8e5a70e3ab".parse().unwrap();
        let as_pypi_req: PyPiRequirement = pypi.try_into().unwrap();
        assert_eq!(
            as_pypi_req,
            PyPiRequirement::Git {
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
        let as_pypi_req: PyPiRequirement = pypi.try_into().unwrap();
        assert_eq!(as_pypi_req, PyPiRequirement::Url { url: Url::parse("https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl").unwrap(), extras: vec![], subdirectory: None });

        let pypi: Requirement = "boltons[nichita] @ https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl".parse().unwrap();
        let as_pypi_req: PyPiRequirement = pypi.try_into().unwrap();
        assert_eq!(as_pypi_req, PyPiRequirement::Url { url: Url::parse("https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl").unwrap(), extras: vec![ExtraName::new("nichita".to_string()).unwrap()], subdirectory: None });

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

    #[test]
    fn test_pep508_git_url() {
        let parsed = pep508_rs::Requirement::from_str(
            "attrs @ git+ssh://git@github.com/python-attrs/attrs.git@main",
        )
        .unwrap();
        assert_eq!(
            PyPiRequirement::try_from(parsed).unwrap(),
            PyPiRequirement::Git {
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
            PyPiRequirement::try_from(parsed).unwrap(),
            PyPiRequirement::Git {
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
            let req: PyPiRequirement = th.required("pkg").unwrap();
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
            let req = th.required::<PyPiRequirement>("pkg").unwrap_err();

            let result = json!({
                "error": format_parse_error(input, TomlError::TomlError(req))
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
