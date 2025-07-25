use crate::{PixiPypiSpec, VersionOrStar};
use itertools::Itertools;
use pep508_rs::ExtraName;
use pixi_spec::{GitReference, GitSpec};
use pixi_toml::{TomlFromStr, TomlWith};
use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;
use thiserror::Error;
use toml_span::de_helpers::TableHelper;
use toml_span::{DeserError, Value};
use url::Url;

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

impl RawPyPiRequirement {
    fn into_pypi_requirement(self) -> Result<PixiPypiSpec, SpecConversion> {
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
            (Some(url), None, None, extras, None) => PixiPypiSpec::Url {
                url,
                extras,
                subdirectory: self.subdirectory,
            },
            (None, Some(path), None, extras, None) => PixiPypiSpec::Path {
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
                PixiPypiSpec::Git {
                    url: GitSpec {
                        git,
                        rev,
                        subdirectory: self.subdirectory,
                    },
                    extras,
                }
            }
            (None, None, None, extras, index) => PixiPypiSpec::Version {
                version: self.version.unwrap_or(VersionOrStar::Star),
                extras,
                index,
            },
            (_, _, _, extras, index) if !extras.is_empty() => PixiPypiSpec::Version {
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

impl<'de> toml_span::Deserialize<'de> for PixiPypiSpec {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        if let Some(str) = value.as_str() {
            return Ok(PixiPypiSpec::RawVersion(
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

impl From<PixiPypiSpec> for toml_edit::Value {
    /// PyPiRequirement to a toml_edit item, to put in the manifest file.
    fn from(val: PixiPypiSpec) -> toml_edit::Value {
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
            PixiPypiSpec::Version {
                version,
                extras,
                index,
            } if extras.is_empty() && index.is_none() => {
                toml_edit::Value::from(version.to_string())
            }
            PixiPypiSpec::Version {
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
            PixiPypiSpec::Git {
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
            PixiPypiSpec::Path {
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
            PixiPypiSpec::Url {
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
            PixiPypiSpec::RawVersion(version) => {
                toml_edit::Value::String(toml_edit::Formatted::new(version.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::PixiPypiSpec;
    use insta::assert_snapshot;
    use pep508_rs::ExtraName;
    use pixi_spec::{GitReference, GitSpec};
    use pixi_test_utils::format_parse_error;
    use pixi_toml::TomlIndexMap;
    use std::{path::PathBuf, str::FromStr};
    use toml_span::{Deserialize, value::ValueInner};
    use url::Url;

    fn from_toml_str<T: for<'de> toml_span::Deserialize<'de>>(
        input: &str,
    ) -> Result<T, pixi_toml::TomlDiagnostic> {
        let mut toml_value = toml_span::parse(input)?;
        toml_span::Deserialize::deserialize(&mut toml_value).map_err(|deser_err| {
            deser_err
                .errors
                .into_iter()
                .next()
                .expect("empty deser error")
                .into()
        })
    }

    #[test]
    fn test_only_version() {
        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
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
            &PixiPypiSpec::RawVersion(">=3.12".parse().unwrap())
        );

        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
            r#"foo = "==3.12.0""#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PixiPypiSpec::RawVersion("==3.12.0".parse().unwrap())
        );

        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
            r#"foo = "~=2.1.3""#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PixiPypiSpec::RawVersion("~=2.1.3".parse().unwrap())
        );

        let requirement =
            from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(r#"foo = "*""#)
                .unwrap()
                .into_inner();
        assert_eq!(requirement.first().unwrap().1, &PixiPypiSpec::default());
    }

    #[test]
    fn test_extended() {
        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
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
            &PixiPypiSpec::Version {
                version: ">=3.12".parse().unwrap(),
                extras: vec![ExtraName::from_str("bar").unwrap()],
                index: None,
            }
        );

        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
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
            &PixiPypiSpec::Version {
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
        let pypi_requirement = from_toml_str::<PixiPypiSpec>(
            r#"
        version = "==1.2.3"
        extras = ["feature1", "feature2"]
        "#,
        )
        .unwrap();

        assert_eq!(
            pypi_requirement,
            PixiPypiSpec::Version {
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
        let pypi_requirement = PixiPypiSpec::deserialize(&mut toml_span::Value::new(
            ValueInner::String(r#"==1.2.3"#.into()),
        ))
        .unwrap();
        assert_eq!(
            pypi_requirement,
            PixiPypiSpec::RawVersion("==1.2.3".parse().unwrap())
        );
    }

    #[test]
    fn test_deserialize_pypi_requirement_from_str_with_star() {
        let pypi_requirement =
            PixiPypiSpec::deserialize(&mut toml_span::Value::new(ValueInner::String("*".into())))
                .unwrap();
        assert_eq!(pypi_requirement, PixiPypiSpec::default());
    }

    #[test]
    fn test_deserialize_pypi_from_path() {
        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
            r#"foo = { path = "../numpy-test" }"#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PixiPypiSpec::Path {
                path: PathBuf::from("../numpy-test"),
                editable: None,
                extras: vec![],
            },
        );
    }
    #[test]
    fn test_deserialize_pypi_from_path_editable() {
        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
            r#"foo = { path = "../numpy-test", editable = true }"#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PixiPypiSpec::Path {
                path: PathBuf::from("../numpy-test"),
                editable: Some(true),
                extras: vec![],
            }
        );
    }

    #[test]
    fn test_deserialize_fail_on_unknown() {
        let input = r#"foo = { borked = "bork"}"#;
        assert_snapshot!(format_parse_error(input, from_toml_str::<TomlIndexMap::<pep508_rs::PackageName, PixiPypiSpec>>(input).unwrap_err()), @r###"
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
        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
            r#"foo = { url = "https://test.url.com"}"#,
        )
        .unwrap()
        .into_inner();

        assert_eq!(
            requirement.first().unwrap().1,
            &PixiPypiSpec::Url {
                url: Url::parse("https://test.url.com").unwrap(),
                extras: vec![],
                subdirectory: None,
            }
        );
    }

    #[test]
    fn test_deserialize_pypi_from_git() {
        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
            r#"foo = { git = "https://test.url.git" }"#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PixiPypiSpec::Git {
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
        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
            r#"foo = { git = "https://test.url.git", branch = "main" }"#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PixiPypiSpec::Git {
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
        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
            r#"foo = { git = "https://test.url.git", tag = "v.1.2.3" }"#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PixiPypiSpec::Git {
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
        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
            r#"flask = { git = "https://github.com/pallets/flask.git", tag = "3.0.0"}"#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PixiPypiSpec::Git {
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
        let requirement = from_toml_str::<TomlIndexMap<pep508_rs::PackageName, PixiPypiSpec>>(
            r#"foo = { git = "https://test.url.git", rev = "123456" }"#,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            requirement.first().unwrap().1,
            &PixiPypiSpec::Git {
                url: GitSpec {
                    git: Url::parse("https://test.url.git").unwrap(),
                    rev: Some(GitReference::Rev("123456".to_string())),
                    subdirectory: None,
                },
                extras: vec![],
            }
        );
    }
}
