use std::{
    fmt,
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
};

use itertools::Itertools;
use pep440_rs::VersionSpecifiers;
use pep508_rs::ExtraName;
use pixi_toml::{TomlFromStr, TomlWith};
use serde::{de::Error, Deserialize, Serialize};
use serde_with::serde_as;
use thiserror::Error;
use toml_span::{de_helpers::TableHelper, DeserError, Value};
use url::Url;

use super::{pypi_requirement_types::GitRevParseError, GitRev, VersionOrStar};
use crate::{utils::extract_directory_from_url, PyPiRequirement::RawVersion};

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ParsedGitUrl {
    pub git: Url,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub rev: Option<GitRev>,
    pub subdirectory: Option<String>,
}

impl TryFrom<Url> for ParsedGitUrl {
    type Error = Pep508ToPyPiRequirementError;

    fn try_from(url: Url) -> Result<Self, Self::Error> {
        let subdirectory = extract_directory_from_url(&url);

        // Strip the git+ from the url.
        let url_without_git = url.as_str().strip_prefix("git+").unwrap_or(url.as_str());
        let mut url = Url::parse(url_without_git)?;
        url.set_fragment(None);

        // Split the repository url and the rev.
        let (repository_url, rev) = if let Some((prefix, suffix)) = url
            .path()
            .rsplit_once('@')
            .map(|(prefix, suffix)| (prefix.to_string(), suffix.to_string()))
        {
            let mut repository_url = url.clone();
            repository_url.set_path(&prefix);
            (repository_url, Some(GitRev::from_str(&suffix)?))
        } else {
            (url, None)
        };

        Ok(ParsedGitUrl {
            git: repository_url,
            branch: None,
            tag: None,
            rev,
            subdirectory,
        })
    }
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(untagged, rename_all = "kebab-case", deny_unknown_fields)]
pub enum PyPiRequirement {
    Git {
        #[serde(flatten)]
        url: ParsedGitUrl,
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
        return Some(format!("it seems you're trying to add a path dependency, please specify as a table with a `path` key: '{{ path = \"{input}\" }}'"));
    }

    if input.contains("git") {
        return Some(format!("it seems you're trying to add a git dependency, please specify as a table with a `git` key: '{{ git = \"{input}\" }}'"));
    }

    if input.contains("://") {
        return Some(format!("it seems you're trying to add a url dependency, please specify as a table with a `url` key: '{{ url = \"{input}\" }}'"));
    }

    None
}

#[serde_as]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
struct RawPyPiRequirement {
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub version: Option<VersionOrStar>,

    #[serde(default)]
    extras: Vec<ExtraName>,

    // Path Only
    pub path: Option<PathBuf>,
    pub editable: Option<bool>,

    // Git only
    pub git: Option<Url>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub rev: Option<GitRev>,

    // Url only
    pub url: Option<Url>,

    // Git and Url only
    pub subdirectory: Option<String>,

    // Pinned index
    #[serde(default)]
    pub index: Option<Url>,
}

impl<'de> Deserialize<'de> for PyPiRequirement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .map(|map| {
                let raw_req: RawPyPiRequirement = map.deserialize()?;
                Ok(raw_req
                    .into_pypi_requirement()
                    .map_err(serde_untagged::de::Error::custom)?)
            })
            .string(|s| {
                VersionOrStar::from_str(s).map(RawVersion).map_err(|err| {
                    if let Some(msg) = version_requirement_error(s) {
                        serde_untagged::de::Error::custom(msg)
                    } else {
                        serde_untagged::de::Error::custom(err)
                    }
                })
            })
            .expecting("a version or a mapping with `family` and `version`")
            .deserialize(deserializer)
    }
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
            (None, None, Some(git), extras, None) => PyPiRequirement::Git {
                url: ParsedGitUrl {
                    git,
                    branch: self.branch,
                    tag: self.tag,
                    rev: self.rev,
                    subdirectory: self.subdirectory,
                },
                extras,
            },
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
                    kind: toml_span::ErrorKind::Custom(e.to_string().into()),
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
                    ParsedGitUrl {
                        git,
                        branch,
                        tag,
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

    #[error(transparent)]
    ParseGitRev(#[from] GitRevParseError),

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
                            "git" => Self::Git {
                                url: ParsedGitUrl::try_from(url)?,
                                extras: req.extras,
                            },
                            "bzr" => {
                                return Err(Pep508ToPyPiRequirementError::UnsupportedUrlPrefix {
                                    prefix: prefix.to_string(),
                                    url: u.to_url(),
                                    message: "Bazaar is not supported",
                                })
                            }
                            "hg" => {
                                return Err(Pep508ToPyPiRequirementError::UnsupportedUrlPrefix {
                                    prefix: prefix.to_string(),
                                    url: u.to_url(),
                                    message: "Bazaar is not supported",
                                })
                            }
                            "svn" => {
                                return Err(Pep508ToPyPiRequirementError::UnsupportedUrlPrefix {
                                    prefix: prefix.to_string(),
                                    url: u.to_url(),
                                    message: "Bazaar is not supported",
                                })
                            }
                            _ => {
                                return Err(Pep508ToPyPiRequirementError::UnsupportedUrlPrefix {
                                    prefix: prefix.to_string(),
                                    url: u.to_url(),
                                    message: "Unknown scheme",
                                })
                            }
                        }
                    } else if Path::new(url.path())
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("git"))
                    {
                        let parsed_url = ParsedGitUrl::try_from(url)?;
                        Self::Git {
                            url: parsed_url,
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

    use assert_matches::assert_matches;
    use indexmap::IndexMap;
    use insta::assert_snapshot;
    use pep508_rs::Requirement;
    use serde_json::{json, Value};

    use super::*;
    use crate::pypi::PyPiPackageName;

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
                index: None,
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
                index: None,
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
                index: None,
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
                url: ParsedGitUrl {
                    git: Url::parse("https://test.url.git").unwrap(),
                    branch: None,
                    tag: None,
                    rev: None,
                    subdirectory: None,
                },
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
                url: ParsedGitUrl {
                    git: Url::parse("https://test.url.git").unwrap(),
                    branch: Some("main".to_string()),
                    tag: None,
                    rev: None,
                    subdirectory: None,
                },
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
                url: ParsedGitUrl {
                    git: Url::parse("https://test.url.git").unwrap(),
                    tag: Some("v.1.2.3".to_string()),
                    branch: None,
                    rev: None,
                    subdirectory: None,
                },
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
                url: ParsedGitUrl {
                    git: Url::parse("https://github.com/pallets/flask.git").unwrap(),
                    tag: Some("3.0.0".to_string()),
                    branch: None,
                    rev: None,
                    subdirectory: None,
                },
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
                url: ParsedGitUrl {
                    git: Url::parse("https://test.url.git").unwrap(),
                    rev: Some(GitRev::Short("123456".to_string())),
                    tag: None,
                    branch: None,
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
                url: ParsedGitUrl {
                    git: Url::parse("https://github.com/ecederstrand/exchangelib").unwrap(),
                    branch: None,
                    tag: None,
                    rev: None,
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
                url: ParsedGitUrl {
                    git: Url::parse("https://github.com/ecederstrand/exchangelib").unwrap(),
                    branch: None,
                    tag: None,
                    rev: Some(GitRev::Full(
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
        assert_matches!(
            PyPiRequirement::try_from(parsed),
            Err(Pep508ToPyPiRequirementError::ParseGitRev(
                GitRevParseError::InvalidCharacters(characters)
            )) if characters == "main"
        );

        // With subdirectory
        let parsed = pep508_rs::Requirement::from_str(
            "ribasim@git+https://github.com/Deltares/Ribasim.git#subdirectory=python/ribasim",
        )
        .unwrap();
        assert_eq!(
            PyPiRequirement::try_from(parsed).unwrap(),
            PyPiRequirement::Git {
                url: ParsedGitUrl {
                    git: Url::parse("https://github.com/Deltares/Ribasim.git").unwrap(),
                    branch: None,
                    tag: None,
                    rev: None,
                    subdirectory: Some("python/ribasim".to_string()),
                },
                extras: vec![],
            }
        );
    }

    #[test]
    fn test_deserialize_succeeding() {
        let examples = [
            json! { "==1.2.3" },
            json!({ "version": "==1.2.3" }),
            json! { "*" },
            json!({ "path": "foobar" }),
            json!({ "path": "~/.cache" }),
            json!({ "url": "https://conda.anaconda.org/conda-forge/linux-64/21cmfast-3.3.1-py38h0db86a8_1.conda" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "branch": "main" }),
            json!({ "git": "ssh://github.com/conda-forge/21cmfast-feedstock", "tag": "v1.2.3" }),
            json!({ "git": "https://github.com/prefix-dev/rattler-build", "rev": "123456" }),
        ];

        #[derive(Serialize)]
        struct Snapshot {
            input: Value,
            result: Value,
        }

        let mut snapshot = Vec::new();
        for input in examples {
            let req = serde_json::from_value::<PyPiRequirement>(input.clone()).unwrap();
            let result = serde_json::to_value(&req).unwrap();
            snapshot.push(Snapshot { input, result });
        }

        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_deserialize_failing() {
        let examples = [
            json!({ "ver": "1.2.3" }),
            json!({ "path": "foobar", "version": "==1.2.3" }),
            json!({ "version": "//" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "branch": "main", "tag": "v1" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "branch": "main", "tag": "v1", "rev": "123456" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "branch": "main", "rev": "v1" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "tag": "v1", "rev": "123456" }),
            json!({ "git": "ssh://github.com:conda-forge/21cmfast-feedstock"}),
            json!({ "branch": "main", "tag": "v1", "rev": "123456"  }),
            json! { "/path/style"},
            json! { "./path/style"},
            json! { "\\path\\style"},
            json! { "~/path/style"},
            json! { "https://example.com"},
            json! { "https://github.com/conda-forge/21cmfast-feedstock"},
        ];

        #[derive(Serialize)]
        struct Snapshot {
            input: Value,
            result: Value,
        }

        let mut snapshot = Vec::new();
        for input in examples {
            let error = serde_json::from_value::<PyPiRequirement>(input.clone()).unwrap_err();

            let result = json!({
                "error": format!("ERROR: {error}")
            });

            snapshot.push(Snapshot { input, result });
        }

        insta::assert_yaml_snapshot!(snapshot);
    }
}
