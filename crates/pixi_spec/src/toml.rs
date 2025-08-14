use std::{borrow::Cow, fmt::Display, path::PathBuf};

use itertools::Either;
use pixi_toml::{TomlDigest, TomlFromStr};
use rattler_conda_types::{
    BuildNumberSpec, ChannelConfig, NamedChannelOrUrl, NamelessMatchSpec,
    ParseStrictness::{Lenient, Strict},
    StringMatcher, VersionSpec,
    version_spec::{ParseConstraintError, ParseVersionSpecError},
};
use rattler_digest::{Md5Hash, Sha256Hash};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use serde_with::serde_as;
use thiserror::Error;
use toml_span::{
    DeserError, ErrorKind, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};
use url::Url;

use crate::{
    BinarySpec, DetailedSpec, GitReference, GitSpec, PathSourceSpec, PathSpec, PixiSpec,
    SourceLocationSpec, UrlSourceSpec, UrlSpec,
};

/// A TOML representation of a package specification.
#[serde_as]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct TomlSpec {
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub version: Option<VersionSpec>,

    /// The source location
    #[serde(flatten)]
    pub location: Option<TomlLocationSpec>,

    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub build: Option<StringMatcher>,

    /// The build number of the package
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub build_number: Option<BuildNumberSpec>,

    /// Match the specific filename of the package
    pub file_name: Option<String>,

    /// The channel of the package
    pub channel: Option<NamedChannelOrUrl>,

    /// The subdir of the channel
    pub subdir: Option<String>,

    /// The license
    pub license: Option<String>,
}

/// A TOML representation of a package source location specification.
#[serde_as]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct TomlLocationSpec {
    /// The URL of the package
    pub url: Option<Url>,

    /// The git url of the package
    pub git: Option<Url>,

    /// The path to the package
    pub path: Option<String>,

    /// The git revision of the package
    pub branch: Option<String>,

    /// The git revision of the package
    pub rev: Option<String>,

    /// The git revision of the package
    pub tag: Option<String>,

    /// The git subdirectory of the package
    pub subdirectory: Option<String>,

    /// The md5 hash of the package
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the package
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,
}

/// Returns a more helpful message when a version spec is used incorrectly.
fn version_spec_error<T: Into<String>>(input: T) -> Option<impl Display> {
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

    if let Ok(match_spec) = NamelessMatchSpec::from_str(&input, Lenient) {
        let spec = PixiSpec::from_nameless_matchspec(
            match_spec,
            &ChannelConfig::default_with_root_dir(PathBuf::default()),
        );
        return Some(format!(
            "expected a version specifier but looks like a matchspec, did you mean {}?",
            spec.to_toml_value()
        ));
    };

    if input.contains("subdir") {
        return Some("it seems you're trying to add a detailed dependency, please specify as a table with a `subdir` key: '{ version = \"<VERSION_SPEC>\", subdir = \"<SUBDIR>\" }'".to_string());
    }

    if input.contains("channel") || input.contains("::") {
        return Some("it seems you're trying to add a detailed dependency, please specify as a table with a `channel` key: '{ version = \"<VERSION_SPEC>\", channel = \"<CHANNEL>\" }'".to_string());
    }

    if input.contains("md5") {
        return Some("it seems you're trying to add a detailed dependency, please specify as a table with a `md5` key: '{ version = \"<VERSION_SPEC>\", md5 = \"<MD5>\" }'".to_string());
    }

    if input.contains("sha256") {
        return Some("it seems you're trying to add a detailed dependency, please specify as a table with a `sha256` key: '{ version = \"<VERSION_SPEC>\", sha256 = \"<SHA256>\" }'".to_string());
    }

    None
}

#[derive(Error, Debug)]
pub enum SpecError {
    #[error("`branch`, `rev`, and `tag` are only valid when `git` is specified")]
    NotAGitSpec,

    #[error("only one of `branch`, `rev`, or `tag` can be specified")]
    MultipleGitRefs,

    #[error(
        "one of `version`, `build`, `build-number`, `file-name`, `channel`, `subdir`, `md5`, `sha256`, `git`, `url`, or `path` must be specified"
    )]
    MissingDetailedIdentifier,

    #[error("only one of `url`, `path`, or `git` can be specified")]
    MultipleIdentifiers,

    #[error("{0} cannot be used with {1}")]
    InvalidCombination(Cow<'static, str>, Cow<'static, str>),

    #[error(transparent)]
    NotABinary(NotBinary),
}

#[derive(Error, Debug)]
pub enum SourceLocationSpecError {
    #[error("`branch`, `rev`, and `tag` are only valid when `git` is specified")]
    NotAGitSpec,

    #[error("only one of `branch`, `rev`, or `tag` can be specified")]
    MultipleGitRefs,

    #[error("only one of `url`, `path`, or `git` can be specified")]
    MultipleIdentifiers,

    #[error("{0} cannot be used with {1}")]
    InvalidCombination(Cow<'static, str>, Cow<'static, str>),

    #[error("must specify one of `path`, `url`, or `git`")]
    NoSourceType,
}

#[derive(Error, Debug)]
pub enum NotBinary {
    #[error("the url does not refer to a valid conda package archive")]
    Url,

    #[error("the path does not refer to a valid conda package archive")]
    Path,

    #[error(
        "`git` can only refer to a source distributions but a binary distribution was expected"
    )]
    Git,
}

impl TomlSpec {
    fn validate_field_combinations(&self) -> Result<(), SpecError> {
        let (is_git, is_path, is_url) = if let Some(loc) = &self.location {
            if loc.git.is_none() && (loc.branch.is_some() || loc.rev.is_some() || loc.tag.is_some())
            {
                return Err(SpecError::NotAGitSpec);
            }
            (loc.git.is_some(), loc.path.is_some(), loc.url.is_some())
        } else {
            (false, false, false)
        };

        let non_detailed_keys = [
            is_git.then_some("`git`"),
            is_path.then_some("`path`"),
            is_url.then_some("`url`"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");

        // Common field checks
        if !non_detailed_keys.is_empty() {
            for (field_name, is_some) in [
                ("`version`", self.version.is_some()),
                ("`build`", self.build.is_some()),
                ("`build_number`", self.build_number.is_some()),
                ("`file_name`", self.file_name.is_some()),
                ("`channel`", self.channel.is_some()),
                ("`subdir`", self.subdir.is_some()),
            ] {
                if is_some {
                    return Err(SpecError::InvalidCombination(
                        field_name.into(),
                        non_detailed_keys.into(),
                    ));
                }
            }
        }

        if let Some(loc) = &self.location {
            let non_url_keys = [is_git.then_some("`git`"), is_path.then_some("`path`")]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ");

            if !non_url_keys.is_empty() {
                for (field_name, is_some) in [
                    ("`sha256`", loc.sha256.is_some()),
                    ("`md5`", loc.md5.is_some()),
                ] {
                    if is_some {
                        return Err(SpecError::InvalidCombination(
                            field_name.into(),
                            non_url_keys.into(),
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Convert the TOML representation into an actual [`PixiSpec`].
    pub fn into_spec(self) -> Result<PixiSpec, SpecError> {
        self.validate_field_combinations()?;

        let spec: PixiSpec;
        if let Some(loc) = self.location {
            spec = match (loc.url, loc.path, loc.git) {
                (Some(url), None, None) => PixiSpec::Url(UrlSpec {
                    url,
                    md5: loc.md5,
                    sha256: loc.sha256,
                }),
                (None, Some(path), None) => PixiSpec::Path(PathSpec { path: path.into() }),
                (None, None, Some(git)) => {
                    let rev = match (loc.branch, loc.rev, loc.tag) {
                        (Some(branch), None, None) => Some(GitReference::Branch(branch)),
                        (None, Some(rev), None) => Some(GitReference::Rev(rev)),
                        (None, None, Some(tag)) => Some(GitReference::Tag(tag)),
                        (None, None, None) => None,
                        _ => {
                            return Err(SpecError::MultipleGitRefs);
                        }
                    };
                    let subdirectory = loc.subdirectory;
                    PixiSpec::Git(GitSpec {
                        git,
                        rev,
                        subdirectory,
                    })
                }
                (None, None, None) => {
                    let is_detailed = self.version.is_some()
                        || self.build.is_some()
                        || self.build_number.is_some()
                        || self.file_name.is_some()
                        || self.channel.is_some()
                        || self.subdir.is_some()
                        || loc.md5.is_some()
                        || loc.sha256.is_some()
                        || self.license.is_some();
                    if !is_detailed {
                        return Err(SpecError::MissingDetailedIdentifier);
                    }

                    PixiSpec::DetailedVersion(Box::new(DetailedSpec {
                        version: self.version,
                        build: self.build,
                        build_number: self.build_number,
                        file_name: self.file_name,
                        channel: self.channel,
                        subdir: self.subdir,
                        md5: loc.md5,
                        sha256: loc.sha256,
                        license: self.license,
                    }))
                }
                (_, _, _) => return Err(SpecError::MultipleIdentifiers),
            };
        } else {
            let is_detailed = self.version.is_some()
                || self.build.is_some()
                || self.build_number.is_some()
                || self.file_name.is_some()
                || self.channel.is_some()
                || self.subdir.is_some()
                || self.license.is_some();
            if !is_detailed {
                return Err(SpecError::MissingDetailedIdentifier);
            }

            spec = PixiSpec::DetailedVersion(Box::new(DetailedSpec {
                version: self.version,
                build: self.build,
                build_number: self.build_number,
                file_name: self.file_name,
                channel: self.channel,
                subdir: self.subdir,
                md5: None,
                sha256: None,
                license: self.license,
            }));
        }

        Ok(spec)
    }

    /// Convert the TOML representation into an actual [`PixiSpec`].
    pub fn into_binary_spec(self) -> Result<BinarySpec, SpecError> {
        self.validate_field_combinations()?;

        let spec: BinarySpec;
        if let Some(loc) = self.location {
            spec = match (loc.url, loc.path, loc.git) {
                (Some(url), None, None) => {
                    let url_spec = UrlSpec {
                        url,
                        md5: loc.md5,
                        sha256: loc.sha256,
                    };
                    if let Either::Right(binary) = url_spec.into_source_or_binary() {
                        BinarySpec::Url(binary)
                    } else {
                        return Err(SpecError::NotABinary(NotBinary::Url));
                    }
                }
                (None, Some(path), None) => {
                    let path_spec = PathSpec { path: path.into() };
                    if let Either::Right(binary) = path_spec.into_source_or_binary() {
                        BinarySpec::Path(binary)
                    } else {
                        return Err(SpecError::NotABinary(NotBinary::Path));
                    }
                }
                (None, None, Some(_git)) => {
                    return Err(SpecError::NotABinary(NotBinary::Git));
                }
                (None, None, None) => {
                    let is_detailed = self.version.is_some()
                        || self.build.is_some()
                        || self.build_number.is_some()
                        || self.file_name.is_some()
                        || self.channel.is_some()
                        || self.subdir.is_some()
                        || loc.md5.is_some()
                        || loc.sha256.is_some()
                        || self.license.is_some();
                    if !is_detailed {
                        return Err(SpecError::MissingDetailedIdentifier);
                    }

                    BinarySpec::DetailedVersion(Box::new(DetailedSpec {
                        version: self.version,
                        build: self.build,
                        build_number: self.build_number,
                        file_name: self.file_name,
                        channel: self.channel,
                        subdir: self.subdir,
                        md5: loc.md5,
                        sha256: loc.sha256,
                        license: self.license,
                    }))
                }
                (_, _, _) => return Err(SpecError::MultipleIdentifiers),
            };
        } else {
            let is_detailed = self.version.is_some()
                || self.build.is_some()
                || self.build_number.is_some()
                || self.file_name.is_some()
                || self.channel.is_some()
                || self.subdir.is_some()
                || self.license.is_some();
            if !is_detailed {
                return Err(SpecError::MissingDetailedIdentifier);
            }

            spec = BinarySpec::DetailedVersion(Box::new(DetailedSpec {
                version: self.version,
                build: self.build,
                build_number: self.build_number,
                file_name: self.file_name,
                channel: self.channel,
                subdir: self.subdir,
                md5: None,
                sha256: None,
                license: self.license,
            }));
        };
        Ok(spec)
    }
}

impl TomlLocationSpec {
    fn validate_field_combinations(&self) -> Result<(), SourceLocationSpecError> {
        let (is_git, is_path, is_url) = {
            if self.git.is_none()
                && (self.branch.is_some() || self.rev.is_some() || self.tag.is_some())
            {
                return Err(SourceLocationSpecError::NotAGitSpec);
            }
            (self.git.is_some(), self.path.is_some(), self.url.is_some())
        };

        if !is_git && !is_path && !is_url {
            return Err(SourceLocationSpecError::NoSourceType);
        }

        let non_url_keys = [is_git.then_some("`git`"), is_path.then_some("`path`")]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ");

        if !non_url_keys.is_empty() {
            for (field_name, is_some) in [
                ("`sha256`", self.sha256.is_some()),
                ("`md5`", self.md5.is_some()),
            ] {
                if is_some {
                    return Err(SourceLocationSpecError::InvalidCombination(
                        field_name.into(),
                        non_url_keys.into(),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Convert the TOML representation into a `SourceLocationSpec`.
    pub fn into_source_location_spec(self) -> Result<SourceLocationSpec, SourceLocationSpecError> {
        self.validate_field_combinations()?;

        let spec = match (self.url, self.path, self.git) {
            (Some(url), None, None) => SourceLocationSpec::Url(UrlSourceSpec {
                url,
                md5: self.md5,
                sha256: self.sha256,
            }),
            (None, Some(path), None) => {
                SourceLocationSpec::Path(PathSourceSpec { path: path.into() })
            }
            (None, None, Some(git)) => {
                let rev = match (self.branch, self.rev, self.tag) {
                    (Some(branch), None, None) => Some(GitReference::Branch(branch)),
                    (None, Some(rev), None) => Some(GitReference::Rev(rev)),
                    (None, None, Some(tag)) => Some(GitReference::Tag(tag)),
                    (None, None, None) => None,
                    _ => {
                        return Err(SourceLocationSpecError::MultipleGitRefs);
                    }
                };
                let subdirectory = self.subdirectory;
                SourceLocationSpec::Git(GitSpec {
                    git,
                    rev,
                    subdirectory,
                })
            }
            (_, _, _) => return Err(SourceLocationSpecError::MultipleIdentifiers),
        };
        Ok(spec)
    }
}

/// A TOML representation wrapper of a [`VersionSpec`]
/// Used to add custom deserialization for the version spec string
pub struct TomlVersionSpecStr(VersionSpec);

impl TomlVersionSpecStr {
    /// Get inner version spec from the toml wrapper
    pub fn into_inner(self) -> VersionSpec {
        self.0
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlVersionSpecStr {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let str = value.take_string("a version specifier string".into())?;
        parse_version_string(&str)
            .map(TomlVersionSpecStr)
            .map_err(|msg| {
                DeserError::from(toml_span::Error {
                    kind: ErrorKind::Custom(msg.into()),
                    span: value.span,
                    line_info: None,
                })
            })
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlSpec {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let version = th
            .optional::<TomlVersionSpecStr>("version")
            .map(TomlVersionSpecStr::into_inner);
        let url = th
            .optional::<TomlFromStr<_>>("url")
            .map(TomlFromStr::into_inner);
        let git = th
            .optional::<TomlFromStr<_>>("git")
            .map(TomlFromStr::into_inner);
        let path = th.optional("path");
        let branch = th.optional("branch");
        let rev = th.optional("rev");
        let tag = th.optional("tag");
        let subdirectory = th.optional("subdirectory");
        let build = th
            .optional::<TomlFromStr<_>>("build")
            .map(TomlFromStr::into_inner);
        let build_number = th
            .optional::<TomlFromStr<_>>("build-number")
            .map(TomlFromStr::into_inner);
        let file_name = th.optional("file-name");
        let channel = th.optional("channel").map(TomlFromStr::into_inner);
        let subdir = th.optional("subdir");
        let license = th.optional("license");
        let md5 = th
            .optional::<TomlDigest<rattler_digest::Md5>>("md5")
            .map(TomlDigest::into_inner);
        let sha256 = th
            .optional::<TomlDigest<rattler_digest::Sha256>>("sha256")
            .map(TomlDigest::into_inner);

        th.finalize(None)?;

        Ok(TomlSpec {
            version,
            location: Some(TomlLocationSpec {
                url,
                git,
                path,
                branch,
                rev,
                tag,
                subdirectory,
                md5,
                sha256,
            }),
            build,
            build_number,
            file_name,
            channel,
            subdir,
            license,
        })
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlLocationSpec {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let url = th
            .optional::<TomlFromStr<_>>("url")
            .map(TomlFromStr::into_inner);
        let git = th
            .optional::<TomlFromStr<_>>("git")
            .map(TomlFromStr::into_inner);
        let path = th.optional("path");
        let branch = th.optional("branch");
        let rev = th.optional("rev");
        let tag = th.optional("tag");
        let subdirectory = th.optional("subdirectory");
        let md5 = th
            .optional::<TomlDigest<rattler_digest::Md5>>("md5")
            .map(TomlDigest::into_inner);
        let sha256 = th
            .optional::<TomlDigest<rattler_digest::Sha256>>("sha256")
            .map(TomlDigest::into_inner);

        th.finalize(None)?;

        Ok(TomlLocationSpec {
            url,
            git,
            path,
            branch,
            rev,
            tag,
            subdirectory,
            md5,
            sha256,
        })
    }
}

impl<'de> toml_span::Deserialize<'de> for PixiSpec {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(str) => {
                parse_version_string(&str)
                    .map(PixiSpec::Version)
                    .map_err(|msg| {
                        DeserError::from(toml_span::Error {
                            kind: ErrorKind::Custom(msg.into()),
                            span: value.span,
                            line_info: None,
                        })
                    })
            }
            inner @ ValueInner::Table(_) => {
                let mut table_value = Value::with_span(inner, value.span);
                Ok(
                    <TomlSpec as toml_span::Deserialize>::deserialize(&mut table_value)?
                        .into_spec()
                        .map_err(|e| {
                            DeserError::from(toml_span::Error {
                                kind: ErrorKind::Custom(e.to_string().into()),
                                span: table_value.span,
                                line_info: None,
                            })
                        })?,
                )
            }
            inner => Err(expected("a string or a table", inner, value.span).into()),
        }
    }
}

fn parse_version_string(input: &str) -> Result<VersionSpec, String> {
    let err = match VersionSpec::from_str(input, Strict) {
        Ok(ver) => return Ok(ver),
        Err(ParseVersionSpecError::InvalidConstraint(ParseConstraintError::AmbiguousVersion(
            ver,
        ))) => {
            // If we encounter an ambiguous version error, we try to parse it in lenient
            // mode. If that fails, we return the original error.
            match VersionSpec::from_str(input, Lenient) {
                Ok(lenient_version_spec) => {
                    tracing::warn!(
                        "Encountered ambiguous version specifier `{ver}`, could be `{ver}.*` but assuming you meant `=={ver}`. In the future this will result in an error."
                    );
                    return Ok(lenient_version_spec);
                }
                Err(_) => {
                    // Return the original error.
                    ParseVersionSpecError::InvalidConstraint(
                        ParseConstraintError::AmbiguousVersion(ver),
                    )
                }
            }
        }
        Err(e) => e,
    };

    Err(if let Some(msg) = version_spec_error(input) {
        msg.to_string()
    } else {
        err.to_string()
    })
}

impl<'de> Deserialize<'de> for PixiSpec {
    fn deserialize<D>(deserializer: D) -> Result<PixiSpec, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .expecting(
                "a version string like \">=0.9.8\" or a detailed dependency like { version = \">=0.9.8\" }",
            )
            .string(|str| {
                parse_version_string(str)
                    .map(PixiSpec::Version)
                    .map_err(serde_untagged::de::Error::custom)
            })
            .map(|map| {
                let spec: TomlSpec = map.deserialize()?;
                spec.into_spec().map_err(serde_untagged::de::Error::custom)
            })
            .deserialize(deserializer)
    }
}

impl<'de> Deserialize<'de> for PathSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            path: String,
        }

        Raw::deserialize(deserializer).map(|raw| PathSpec {
            path: raw.path.into(),
        })
    }
}

impl Serialize for PathSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Raw {
            path: String,
        }

        Raw {
            path: self.path.to_string(),
        }
        .serialize(serializer)
    }
}

#[cfg(test)]
mod test {
    use serde::Serialize;
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn test_round_trip() {
        let examples = [
            json! { "1.2.3" },
            json!({ "version": "1.2.3" }),
            json!({ "version": "1.2.3", "build-number": ">=3" }),
            json! { "*" },
            json!({ "path": "foobar" }),
            json!({ "path": "~/.cache" }),
            json!({ "subdir": "linux-64" }),
            json!({ "channel": "conda-forge", "subdir": "linux-64" }),
            json!({ "channel": "conda-forge", "subdir": "linux-64" }),
            json!({ "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "version": "1.2.3", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "url": "https://conda.anaconda.org/conda-forge/linux-64/21cmfast-3.3.1-py38h0db86a8_1.conda" }),
            json!({ "url": "https://conda.anaconda.org/conda-forge/linux-64/21cmfast-3.3.1-py38h0db86a8_1.conda", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "branch": "main" }),
            // Errors:
            json!({ "ver": "1.2.3" }),
            json!({ "path": "foobar" , "version": "1.2.3" }),
            json!({ "version": "//" }),
            json!({ "path": "foobar", "version": "//" }),
            json!({ "path": "foobar", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "branch": "main", "tag": "v1" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json! { "/path/style"},
            json! { "./path/style"},
            json! { "\\path\\style"},
            json! { "~/path/style"},
            json! { "https://example.com"},
            json! { "https://github.com/conda-forge/21cmfast-feedstock"},
            json! { "1.2.3[subdir=linux-64]"},
            json! { "1.2.3[channel=conda-forge]"},
            json! { "conda-forge::1.2.3"},
            json! { "1.2.3[md5=315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3]"},
            json! { "1.2.3[sha256=315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3]"},
            json! { "*cpu*"},
            json! { "*=*openblas"},
        ];

        #[derive(Serialize)]
        struct Snapshot {
            input: Value,
            result: Value,
        }

        let mut snapshot = Vec::new();
        for input in examples {
            let spec: Result<PixiSpec, _> = serde_json::from_value(input.clone());
            let result = match spec {
                Ok(spec) => serde_json::to_value(&spec).unwrap(),
                Err(e) => {
                    json!({ "error": format!("ERROR: {e}") })
                }
            };

            snapshot.push(Snapshot { input, result });
        }

        insta::assert_yaml_snapshot!(snapshot);
    }
}
