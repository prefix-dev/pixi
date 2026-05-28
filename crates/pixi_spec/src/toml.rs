use std::{
    borrow::Cow,
    fmt::Display,
    path::{Path, PathBuf},
};

use pixi_toml::{TomlDigest, TomlFromStr, TomlWith, custom_error_message_with_help};
use rattler_conda_types::{
    BuildNumberSpec, ChannelConfig, MatchSpec, MatchSpecCondition, NamedChannelOrUrl,
    NamelessMatchSpec, PackageName, PackageNameMatcher, ParseMatchSpecOptions,
    ParseStrictness::{Lenient, Strict},
    RepodataRevision, StringMatcher, VersionSpec,
    package::CondaArchiveType,
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
    BinarySpec, DetailedSpec, GitReference, GitSpec, MatchspecFields, PathBinarySpec,
    PathSourceSpec, PathSpec, PixiSpec, SourceLocationSpec, Subdirectory, SubdirectoryError,
    UrlBinarySpec, UrlSourceSpec,
};

/// A TOML representation of a package specification.
#[serde_as]
#[derive(Debug, Clone, serde::Deserialize)]
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

    /// Optional extra dependencies to select for the package.
    pub extras: Option<Vec<String>>,

    /// Plain string flags used to select package variants.
    #[serde_as(as = "Option<Vec<serde_with::DisplayFromStr>>")]
    pub flags: Option<Vec<StringMatcher>>,

    /// The channel of the package
    pub channel: Option<NamedChannelOrUrl>,

    /// The subdir of the channel
    pub subdir: Option<String>,

    /// The license
    pub license: Option<String>,

    /// The license family
    pub license_family: Option<String>,

    /// The condition under which this match spec applies.
    pub when: Option<TomlWhen>,

    /// The track features of the package
    pub track_features: Option<Vec<String>>,
}

/// A TOML representation of a package condition.
#[derive(Debug, Clone)]
pub enum TomlWhen {
    /// A package matchspec without bracket or build-string shorthand syntax.
    MatchSpec(String),
    /// All conditions must apply.
    All {
        /// Conditions to combine with a logical AND.
        all: Vec<TomlWhen>,
    },
    /// Any condition may apply.
    Any {
        /// Conditions to combine with a logical OR.
        any: Vec<TomlWhen>,
    },
    /// Expanded package matchspec syntax. Required when matching a build string.
    Expanded(TomlWhenPackage),
}

/// The expanded package condition syntax. Accepts the full set of matchspec
/// fields that [`TomlSpec`] supports, except for source-location fields
/// (`url`, `git`, `path`, `md5`, `sha256`, ...) and `channel` which do not
/// make sense as `when` conditions.
#[derive(Debug, Clone)]
pub struct TomlWhenPackage {
    /// The package name to match.
    pub package: PackageName,
    /// The remaining matchspec fields parsed via [`TomlSpec`]. Boxed to break
    /// the [`TomlWhen`] -> [`TomlSpec`] -> [`TomlWhen`] cycle.
    pub spec: Box<TomlSpec>,
}

const WHEN_SOURCE_LOCATION_ERROR: &str = "source-location fields (`url`, `git`, `path`, `md5`, `sha256`, ...) are not allowed inside `when` tables";
const WHEN_CHANNEL_ERROR: &str = "`channel` is not supported inside `when` tables";

fn validate_when_spec_error(spec: &TomlSpec) -> Option<&'static str> {
    if spec
        .location
        .as_ref()
        .is_some_and(toml_location_spec_has_any_field)
    {
        return Some(WHEN_SOURCE_LOCATION_ERROR);
    }
    if spec.channel.is_some() {
        return Some(WHEN_CHANNEL_ERROR);
    }
    None
}

fn toml_location_spec_has_any_field(loc: &TomlLocationSpec) -> bool {
    loc.url.is_some()
        || loc.git.is_some()
        || loc.path.is_some()
        || loc.branch.is_some()
        || loc.rev.is_some()
        || loc.tag.is_some()
        || loc.subdirectory.is_some()
        || loc.md5.is_some()
        || loc.sha256.is_some()
}

fn toml_spec_has_any_field(spec: &TomlSpec) -> bool {
    spec.version.is_some()
        || spec
            .location
            .as_ref()
            .is_some_and(toml_location_spec_has_any_field)
        || spec.build.is_some()
        || spec.build_number.is_some()
        || spec.file_name.is_some()
        || spec.extras.is_some()
        || spec.flags.is_some()
        || spec.channel.is_some()
        || spec.subdir.is_some()
        || spec.license.is_some()
        || spec.license_family.is_some()
        || spec.when.is_some()
        || spec.track_features.is_some()
}

impl<'de> serde::Deserialize<'de> for TomlWhenPackage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "kebab-case")]
        struct Helper {
            package: PackageName,
            #[serde(flatten)]
            spec: TomlSpec,
        }
        let Helper { package, spec } = Helper::deserialize(deserializer)?;
        let spec = Box::new(spec);
        // Reject location / channel fields the same way the TOML path does so
        // both deserializers stay in sync.
        if let Some(err) = validate_when_spec_error(&spec) {
            return Err(D::Error::custom(err));
        }
        Ok(TomlWhenPackage { package, spec })
    }
}

impl<'de> serde::Deserialize<'de> for TomlWhen {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "kebab-case")]
        struct Helper {
            all: Option<Vec<TomlWhen>>,
            any: Option<Vec<TomlWhen>>,
            package: Option<PackageName>,
            #[serde(flatten)]
            spec: TomlSpec,
        }

        serde_untagged::UntaggedEnumVisitor::new()
            .expecting("a string or a table with `all`, `any`, or `package`")
            .string(|str| {
                parse_when_matchspec(str)
                    .map_err(|err| serde_untagged::de::Error::custom(err.message_with_help()))?;
                Ok(TomlWhen::MatchSpec(str.to_string()))
            })
            .seq(|_| {
                Err(serde_untagged::de::Error::custom(
                    "`when` must be a string or a table with `all`, `any`, or `package`; top-level arrays are not allowed",
                ))
            })
            .map(|map| {
                let Helper {
                    all,
                    any,
                    package,
                    spec,
                } = map.deserialize()?;

                match (all, any, package) {
                    (Some(all), None, None) if !toml_spec_has_any_field(&spec) => {
                        Ok(TomlWhen::All { all })
                    }
                    (None, Some(any), None) if !toml_spec_has_any_field(&spec) => {
                        Ok(TomlWhen::Any { any })
                    }
                    (None, None, Some(package)) => {
                        if let Some(err) = validate_when_spec_error(&spec) {
                            return Err(serde_untagged::de::Error::custom(err));
                        }
                        Ok(TomlWhen::Expanded(TomlWhenPackage {
                            package,
                            spec: Box::new(spec),
                        }))
                    }
                    _ => Err(serde_untagged::de::Error::custom(
                        "`when` tables must contain exactly one of `all`, `any`, or `package`",
                    )),
                }
            })
            .deserialize(deserializer)
    }
}

/// A TOML representation of a package source location specification.
#[serde_as]
#[derive(Debug, Clone, serde::Deserialize)]
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

    if let Ok(match_spec) = NamelessMatchSpec::from_str(
        &input,
        ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
    ) {
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

    #[error("{message}")]
    InvalidWhen {
        message: String,
        help: Option<String>,
    },

    #[error(transparent)]
    NotABinary(NotBinary),

    #[error(transparent)]
    InvalidSubdirectory(#[from] SubdirectoryError),
}

impl SpecError {
    fn message_with_help(&self) -> String {
        match self {
            SpecError::InvalidWhen {
                message,
                help: Some(help),
            } => format!("{message}; {help}"),
            err => err.to_string(),
        }
    }

    fn into_toml_error(self, span: toml_span::Span) -> toml_span::Error {
        let kind = match self {
            SpecError::InvalidWhen {
                message,
                help: Some(help),
            } => ErrorKind::Custom(custom_error_message_with_help(&message, &help).into()),
            err => ErrorKind::Custom(err.to_string().into()),
        };
        toml_span::Error {
            kind,
            span,
            line_info: None,
        }
    }
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

    #[error(transparent)]
    InvalidSubdirectory(#[from] SubdirectoryError),
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

/// Internal tag identifying which kind of `PixiSpec` variant a [`TomlSpec`]
/// resolves to, after inspecting the URL/path extension at parse time.
#[derive(Copy, Clone, Debug)]
enum LocationKind {
    /// No `url` / `path` / `git`.
    None,
    /// A `url = ...` that points at a binary conda archive.
    UrlBinary,
    /// A `url = ...` that points at a non-binary archive (`.zip`, `.tar.gz`,
    /// ...).
    UrlSource,
    /// A `path = ...` that points at a binary conda archive.
    PathBinary,
    /// A `path = ...` that points at a directory or non-binary archive.
    PathSource,
    /// A `git = ...`.
    Git,
}

impl TomlSpec {
    /// Build an empty [`TomlSpec`] with all fields unset.
    pub fn empty() -> Self {
        Self {
            version: None,
            location: None,
            build: None,
            build_number: None,
            file_name: None,
            extras: None,
            flags: None,
            channel: None,
            subdir: None,
            license: None,
            license_family: None,
            when: None,
            track_features: None,
        }
    }

    /// Layer `overrides` on top of `self`. Each non-version field is taken
    /// from `overrides` when set, otherwise from the base. The base owns
    /// `version` (callers must ensure `overrides.version` is `None`).
    pub fn layer_overrides(self, overrides: Self) -> Self {
        let merged_location = match (self.location, overrides.location) {
            (None, m) => m,
            (b, None) => b,
            (Some(b), Some(m)) => Some(TomlLocationSpec {
                url: m.url.or(b.url),
                git: m.git.or(b.git),
                path: m.path.or(b.path),
                branch: m.branch.or(b.branch),
                rev: m.rev.or(b.rev),
                tag: m.tag.or(b.tag),
                subdirectory: m.subdirectory.or(b.subdirectory),
                md5: m.md5.or(b.md5),
                sha256: m.sha256.or(b.sha256),
            }),
        };
        Self {
            version: self.version,
            location: merged_location,
            build: overrides.build.or(self.build),
            build_number: overrides.build_number.or(self.build_number),
            file_name: overrides.file_name.or(self.file_name),
            extras: overrides.extras.or(self.extras),
            flags: overrides.flags.or(self.flags),
            channel: overrides.channel.or(self.channel),
            subdir: overrides.subdir.or(self.subdir),
            license: overrides.license.or(self.license),
            license_family: overrides.license_family.or(self.license_family),
            when: overrides.when.or(self.when),
            track_features: overrides.track_features.or(self.track_features),
        }
    }

    /// Re-base a relative `location.path` from `from_root` to `to_root`.
    /// Absolute paths (detected via `typed_path` for cross-platform safety)
    /// and `~/` paths pass through unchanged. The resulting path always uses
    /// forward slashes so the serialized manifest stays portable across
    /// platforms.
    pub fn rebase_path(&mut self, from_root: &Path, to_root: &Path) {
        let Some(loc) = self.location.as_mut() else {
            return;
        };
        let Some(path) = loc.path.as_ref() else {
            return;
        };
        let typed = typed_path::Utf8TypedPath::derive(path);
        if typed.is_absolute() || path.starts_with("~/") || path.starts_with("~\\") {
            return;
        }
        let absolute = from_root.join(path);
        if let Some(rel) = pathdiff::diff_paths(&absolute, to_root) {
            let s = rel.to_string_lossy().replace('\\', "/");
            loc.path = Some(s);
        }
    }

    /// Parse a `toml_span` value as a [`TomlSpec`]. Accepts either a bare
    /// version string (e.g. `"1.*"`) or a table.
    pub fn deserialize_from_value(value: &mut Value<'_>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(s) => {
                let version = parse_version_string(&s).map_err(|msg| {
                    DeserError::from(toml_span::Error {
                        kind: ErrorKind::Custom(msg.into()),
                        span: value.span,
                        line_info: None,
                    })
                })?;
                Ok(Self {
                    version: Some(version),
                    ..Self::empty()
                })
            }
            inner @ ValueInner::Table(_) => {
                let mut tbl = Value::with_span(inner, value.span);
                <Self as toml_span::Deserialize>::deserialize(&mut tbl)
            }
            other => Err(expected("a string or a table", other, value.span).into()),
        }
    }

    /// Validate combinations of fields once the binary-vs-source nature of
    /// the location has been decided.
    ///
    /// Rules:
    /// * `branch` / `rev` / `tag` only with `git`.
    /// * `sha256` / `md5` only with `url`.
    /// * Only one of `url` / `path` / `git`.
    /// * `channel` / `file-name` are forbidden with any source location.
    /// * Matchspec fields (`version`, `build`, …) are forbidden when the URL
    ///   or path resolves to a *binary* archive — the resulting package is
    ///   already fully specified — but allowed with source locations.
    fn validate_field_combinations(&self) -> Result<LocationKind, SpecError> {
        let location_kind = if let Some(loc) = &self.location {
            if loc.git.is_none() && (loc.branch.is_some() || loc.rev.is_some() || loc.tag.is_some())
            {
                return Err(SpecError::NotAGitSpec);
            }
            match (loc.url.as_ref(), loc.path.as_ref(), loc.git.as_ref()) {
                (Some(url), None, None) => {
                    if url
                        .path_segments()
                        .and_then(Iterator::last)
                        .and_then(|seg| CondaArchiveType::try_from(Path::new(seg)))
                        .is_some()
                    {
                        LocationKind::UrlBinary
                    } else {
                        LocationKind::UrlSource
                    }
                }
                (None, Some(path), None) => {
                    if CondaArchiveType::try_from(Path::new(path.as_str())).is_some() {
                        LocationKind::PathBinary
                    } else {
                        LocationKind::PathSource
                    }
                }
                (None, None, Some(_)) => LocationKind::Git,
                (None, None, None) => LocationKind::None,
                _ => return Err(SpecError::MultipleIdentifiers),
            }
        } else {
            LocationKind::None
        };

        // `channel` and `file-name` are always forbidden with a source
        // location (binary or otherwise).
        let source_or_binary_keys = match location_kind {
            LocationKind::Git => "`git`",
            LocationKind::UrlSource | LocationKind::UrlBinary => "`url`",
            LocationKind::PathSource | LocationKind::PathBinary => "`path`",
            LocationKind::None => "",
        };
        if !source_or_binary_keys.is_empty() {
            for (field_name, is_some) in [
                ("`channel`", self.channel.is_some()),
                ("`file-name`", self.file_name.is_some()),
            ] {
                if is_some {
                    return Err(SpecError::InvalidCombination(
                        field_name.into(),
                        source_or_binary_keys.into(),
                    ));
                }
            }
        }

        // Matchspec fields are rejected when the location resolves to a
        // binary archive: a `.conda` archive is already a fully-specified
        // package, so `version` etc. would be meaningless.
        let binary_location_key = match location_kind {
            LocationKind::UrlBinary => Some("`url`"),
            LocationKind::PathBinary => Some("`path`"),
            _ => None,
        };
        if let Some(key) = binary_location_key {
            for (field_name, is_some) in [
                ("`version`", self.version.is_some()),
                ("`build`", self.build.is_some()),
                ("`build-number`", self.build_number.is_some()),
                ("`extras`", self.extras.is_some()),
                ("`flags`", self.flags.is_some()),
                ("`subdir`", self.subdir.is_some()),
                ("`license`", self.license.is_some()),
                ("`license-family`", self.license_family.is_some()),
                ("`when`", self.when.is_some()),
                ("`track-features`", self.track_features.is_some()),
            ] {
                if is_some {
                    return Err(SpecError::InvalidCombination(field_name.into(), key.into()));
                }
            }
        }

        // `sha256` / `md5` only apply to URL specs (binary or source archive).
        if let Some(loc) = &self.location {
            let non_url_keys = match location_kind {
                LocationKind::Git => "`git`",
                LocationKind::PathSource | LocationKind::PathBinary => "`path`",
                _ => "",
            };

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

        Ok(location_kind)
    }

    /// Convert the TOML representation into an actual [`PixiSpec`].
    pub fn into_spec(self) -> Result<PixiSpec, SpecError> {
        let kind = self.validate_field_combinations()?;
        let condition = self.when.map(TomlWhen::into_condition).transpose()?;
        let matchspec = MatchspecFields {
            version: self.version,
            build: self.build,
            build_number: self.build_number,
            extras: self.extras,
            flags: self.flags,
            subdir: self.subdir,
            license: self.license,
            license_family: self.license_family,
            condition,
            track_features: self.track_features,
        };

        match kind {
            LocationKind::None => {
                let (md5, sha256) = match &self.location {
                    Some(loc) => (loc.md5, loc.sha256),
                    None => (None, None),
                };
                let any_field = !matchspec.is_empty()
                    || self.file_name.is_some()
                    || self.channel.is_some()
                    || md5.is_some()
                    || sha256.is_some();
                if !any_field {
                    return Err(SpecError::MissingDetailedIdentifier);
                }
                Ok(PixiSpec::Detailed(Box::new(DetailedSpec {
                    version: matchspec.version,
                    build: matchspec.build,
                    build_number: matchspec.build_number,
                    file_name: self.file_name,
                    extras: matchspec.extras,
                    flags: matchspec.flags,
                    channel: self.channel,
                    subdir: matchspec.subdir,
                    md5,
                    sha256,
                    license: matchspec.license,
                    license_family: matchspec.license_family,
                    condition: matchspec.condition,
                    track_features: matchspec.track_features,
                })))
            }
            LocationKind::UrlBinary => {
                let loc = self.location.expect("location set when kind != None");
                let url = loc.url.expect("url set for UrlBinary kind");
                Ok(PixiSpec::UrlBinary(UrlBinarySpec {
                    url,
                    md5: loc.md5,
                    sha256: loc.sha256,
                }))
            }
            LocationKind::UrlSource => {
                let loc = self.location.expect("location set when kind != None");
                let url = loc.url.expect("url set for UrlSource kind");
                let subdirectory = loc
                    .subdirectory
                    .map(Subdirectory::try_from)
                    .transpose()?
                    .unwrap_or_default();
                Ok(PixiSpec::UrlSource(Box::new(UrlSourceSpec {
                    url,
                    md5: loc.md5,
                    sha256: loc.sha256,
                    subdirectory,
                    matchspec,
                })))
            }
            LocationKind::PathBinary => {
                let loc = self.location.expect("location set when kind != None");
                let path = loc.path.expect("path set for PathBinary kind");
                Ok(PixiSpec::PathBinary(PathBinarySpec { path: path.into() }))
            }
            LocationKind::PathSource => {
                let loc = self.location.expect("location set when kind != None");
                let path = loc.path.expect("path set for PathSource kind");
                Ok(PixiSpec::PathSource(Box::new(PathSourceSpec {
                    path: path.into(),
                    matchspec,
                })))
            }
            LocationKind::Git => {
                let loc = self.location.expect("location set when kind != None");
                let git = loc.git.expect("git set for Git kind");
                let rev = match (loc.branch, loc.rev, loc.tag) {
                    (Some(branch), None, None) => Some(GitReference::Branch(branch)),
                    (None, Some(rev), None) => Some(GitReference::Rev(rev)),
                    (None, None, Some(tag)) => Some(GitReference::Tag(tag)),
                    (None, None, None) => None,
                    _ => return Err(SpecError::MultipleGitRefs),
                };
                let subdirectory = loc
                    .subdirectory
                    .map(Subdirectory::try_from)
                    .transpose()?
                    .unwrap_or_default();
                Ok(PixiSpec::Git(Box::new(GitSpec {
                    git,
                    rev,
                    subdirectory,
                    matchspec,
                })))
            }
        }
    }

    /// Convert the TOML representation into a [`BinarySpec`].
    pub fn into_binary_spec(self) -> Result<BinarySpec, SpecError> {
        let kind = self.validate_field_combinations()?;
        let condition = self.when.map(TomlWhen::into_condition).transpose()?;

        match kind {
            LocationKind::None => {
                let (md5, sha256) = match &self.location {
                    Some(loc) => (loc.md5, loc.sha256),
                    None => (None, None),
                };
                Ok(BinarySpec::DetailedVersion(Box::new(DetailedSpec {
                    version: self.version,
                    build: self.build,
                    build_number: self.build_number,
                    file_name: self.file_name,
                    extras: self.extras,
                    flags: self.flags,
                    channel: self.channel,
                    subdir: self.subdir,
                    md5,
                    sha256,
                    license: self.license,
                    license_family: self.license_family,
                    condition,
                    track_features: self.track_features,
                })))
            }
            LocationKind::UrlBinary => {
                let loc = self.location.expect("location set");
                let url = loc.url.expect("url set");
                Ok(BinarySpec::Url(UrlBinarySpec {
                    url,
                    md5: loc.md5,
                    sha256: loc.sha256,
                }))
            }
            LocationKind::PathBinary => {
                let loc = self.location.expect("location set");
                let path = loc.path.expect("path set");
                Ok(BinarySpec::Path(PathBinarySpec { path: path.into() }))
            }
            LocationKind::UrlSource => Err(SpecError::NotABinary(NotBinary::Url)),
            LocationKind::PathSource => Err(SpecError::NotABinary(NotBinary::Path)),
            LocationKind::Git => Err(SpecError::NotABinary(NotBinary::Git)),
        }
    }
}

impl TomlWhen {
    fn into_condition(self) -> Result<MatchSpecCondition, SpecError> {
        match self {
            TomlWhen::MatchSpec(spec) => parse_when_matchspec(&spec),
            TomlWhen::All { all } => fold_when_conditions(all, MatchSpecCondition::And, "all"),
            TomlWhen::Any { any } => fold_when_conditions(any, MatchSpecCondition::Or, "any"),
            TomlWhen::Expanded(expanded) => Ok(MatchSpecCondition::MatchSpec(Box::new(
                expanded.into_match_spec()?,
            ))),
        }
    }
}

impl TomlWhenPackage {
    fn into_match_spec(self) -> Result<MatchSpec, SpecError> {
        // Build the matchspec from the inner TomlSpec fields. Source-location
        // and channel fields were already rejected at parse time by
        // `validate_when_spec`.
        let TomlWhenPackage { package, spec } = self;
        let nested_condition = spec.when.map(TomlWhen::into_condition).transpose()?;
        Ok(MatchSpec {
            name: PackageNameMatcher::Exact(package),
            version: spec.version,
            build: spec.build,
            build_number: spec.build_number,
            file_name: spec.file_name,
            extras: spec.extras,
            flags: spec.flags,
            subdir: spec.subdir,
            license: spec.license,
            license_family: spec.license_family,
            track_features: spec.track_features,
            condition: nested_condition,
            ..MatchSpec::default()
        })
    }
}

/// Validate that a [`TomlSpec`] used inside an expanded `when` table only
/// contains matchspec fields. Source-location fields (`url`, `git`, `path`,
/// `md5`, `sha256`, ...) and `channel` are rejected — the former because they
/// have no meaning as conditions, the latter because resolving it to a
/// [`rattler_conda_types::Channel`] needs a [`ChannelConfig`] that isn't
/// available at parse time.
fn validate_when_spec(spec: &TomlSpec, span: toml_span::Span) -> Result<(), DeserError> {
    if let Some(err) = validate_when_spec_error(spec) {
        return Err(DeserError::from(toml_span::Error {
            kind: ErrorKind::Custom(err.into()),
            span,
            line_info: None,
        }));
    }
    Ok(())
}

fn toml_string_literal(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn toml_string_array_literal<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    format!(
        "[{}]",
        values
            .into_iter()
            .map(toml_string_literal)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn expanded_when_matchspec(match_spec: &MatchSpec) -> Option<String> {
    let package = match_spec.name.as_exact()?.as_source();
    let mut fields = vec![format!("package = {}", toml_string_literal(package))];

    if let Some(version) = &match_spec.version {
        fields.push(format!(
            "version = {}",
            toml_string_literal(&version.to_string())
        ));
    }
    if let Some(build) = &match_spec.build {
        fields.push(format!(
            "build = {}",
            toml_string_literal(&build.to_string())
        ));
    }
    if let Some(build_number) = &match_spec.build_number {
        fields.push(format!(
            "build-number = {}",
            toml_string_literal(&build_number.to_string())
        ));
    }
    if let Some(file_name) = &match_spec.file_name {
        fields.push(format!("file-name = {}", toml_string_literal(file_name)));
    }
    if let Some(extras) = &match_spec.extras {
        fields.push(format!(
            "extras = {}",
            toml_string_array_literal(extras.iter().map(String::as_str))
        ));
    }
    if let Some(flags) = &match_spec.flags {
        let flags = flags.iter().map(ToString::to_string).collect::<Vec<_>>();
        fields.push(format!(
            "flags = {}",
            toml_string_array_literal(flags.iter().map(String::as_str))
        ));
    }
    if let Some(channel) = &match_spec.channel {
        fields.push(format!("channel = {}", toml_string_literal(channel.name())));
    }
    if let Some(subdir) = &match_spec.subdir {
        fields.push(format!("subdir = {}", toml_string_literal(subdir)));
    }
    if let Some(md5) = &match_spec.md5 {
        fields.push(format!(
            "md5 = {}",
            toml_string_literal(&format!("{md5:x}"))
        ));
    }
    if let Some(sha256) = &match_spec.sha256 {
        fields.push(format!(
            "sha256 = {}",
            toml_string_literal(&format!("{sha256:x}"))
        ));
    }
    if let Some(url) = &match_spec.url {
        fields.push(format!("url = {}", toml_string_literal(url.as_str())));
    }
    if let Some(license) = &match_spec.license {
        fields.push(format!("license = {}", toml_string_literal(license)));
    }
    if let Some(license_family) = &match_spec.license_family {
        fields.push(format!(
            "license-family = {}",
            toml_string_literal(license_family)
        ));
    }
    if let Some(track_features) = &match_spec.track_features {
        fields.push(format!(
            "track-features = {}",
            toml_string_array_literal(track_features.iter().map(String::as_str))
        ));
    }

    Some(format!("when = {{ {} }}", fields.join(", ")))
}

fn invalid_when(message: impl Into<String>) -> SpecError {
    SpecError::InvalidWhen {
        message: message.into(),
        help: None,
    }
}

fn invalid_when_with_help(message: impl Into<String>, help: impl Into<String>) -> SpecError {
    SpecError::InvalidWhen {
        message: message.into(),
        help: Some(help.into()),
    }
}

fn parse_when_matchspec(input: &str) -> Result<MatchSpecCondition, SpecError> {
    let match_spec = MatchSpec::from_str(
        input,
        ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
    )
    .map_err(|err| invalid_when(format!("invalid `when` matchspec: {err}")))?;

    if match_spec.name.as_exact().is_none() {
        return Err(invalid_when("`when` strings must name an exact package"));
    }

    let expanded = expanded_when_matchspec(&match_spec);

    if input.contains(['[', ']']) {
        let message = "`when` strings do not support bracket matchspec syntax";
        return Err(match expanded {
            Some(expanded) => {
                invalid_when_with_help(message, format!("use the expanded form `{expanded}`"))
            }
            None => invalid_when(message),
        });
    }

    if match_spec.build.is_some() {
        let message = "`when` strings do not support build-string shorthand";
        return Err(match expanded {
            Some(expanded) => {
                invalid_when_with_help(message, format!("use the expanded form `{expanded}`"))
            }
            None => invalid_when(message),
        });
    }

    if match_spec.channel.is_some() {
        let message = "`when` strings do not support channel prefixes";
        return Err(match expanded {
            Some(expanded) => invalid_when_with_help(
                message,
                format!(
                    "this would be `{expanded}`, but `channel` is not supported inside `when` tables"
                ),
            ),
            None => invalid_when(message),
        });
    }

    if match_spec.build_number.is_some()
        || match_spec.file_name.is_some()
        || match_spec.subdir.is_some()
        || match_spec.md5.is_some()
        || match_spec.sha256.is_some()
        || match_spec.url.is_some()
        || match_spec.license.is_some()
        || match_spec.license_family.is_some()
        || match_spec.extras.is_some()
        || match_spec.flags.is_some()
        || match_spec.condition.is_some()
        || match_spec.track_features.is_some()
        || match_spec.namespace.is_some()
    {
        let message = "`when` strings only support package names with optional version constraints";
        return Err(match expanded {
            Some(expanded) => {
                invalid_when_with_help(message, format!("use the expanded form `{expanded}`"))
            }
            None => invalid_when(message),
        });
    }

    Ok(MatchSpecCondition::MatchSpec(Box::new(match_spec)))
}

fn fold_when_conditions(
    conditions: Vec<TomlWhen>,
    combine: fn(Box<MatchSpecCondition>, Box<MatchSpecCondition>) -> MatchSpecCondition,
    field: &'static str,
) -> Result<MatchSpecCondition, SpecError> {
    let mut conditions = conditions.into_iter().map(TomlWhen::into_condition);
    let first = conditions.next().ok_or_else(|| {
        invalid_when(format!(
            "`when.{field}` must contain at least one condition"
        ))
    })??;

    conditions.try_fold(first, |left, right| {
        Ok(combine(Box::new(left), Box::new(right?)))
    })
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
                subdirectory: self
                    .subdirectory
                    .map(Subdirectory::try_from)
                    .transpose()?
                    .unwrap_or_default(),
                matchspec: MatchspecFields::default(),
            }),
            (None, Some(path), None) => SourceLocationSpec::Path(PathSourceSpec {
                path: path.into(),
                matchspec: MatchspecFields::default(),
            }),
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
                let subdirectory = self
                    .subdirectory
                    .map(Subdirectory::try_from)
                    .transpose()?
                    .unwrap_or_default();
                SourceLocationSpec::Git(GitSpec {
                    git,
                    rev,
                    subdirectory,
                    matchspec: MatchspecFields::default(),
                })
            }
            (_, _, _) => return Err(SourceLocationSpecError::MultipleIdentifiers),
        };
        Ok(spec)
    }
}

/// A TOML representation wrapper of a [`VersionSpec`]
/// Used to add custom deserialization for the version spec string
#[derive(Debug, Clone)]
pub struct TomlVersionSpecStr(VersionSpec);

impl TomlVersionSpecStr {
    /// Get inner version spec from the toml wrapper
    pub fn into_inner(self) -> VersionSpec {
        self.0
    }
}

impl<'de> serde::Deserialize<'de> for TomlVersionSpecStr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let str = String::deserialize(deserializer)?;
        parse_version_string(&str)
            .map(Self)
            .map_err(serde::de::Error::custom)
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
        let extras = th.optional::<Vec<String>>("extras");
        let flags = th
            .optional::<TomlWith<_, Vec<TomlFromStr<StringMatcher>>>>("flags")
            .map(TomlWith::into_inner);
        let channel = th.optional("channel").map(TomlFromStr::into_inner);
        let subdir = th.optional("subdir");
        let license = th.optional("license");
        let license_family = th.optional("license-family");
        let when = th.optional("when");
        let track_features = th.optional::<Vec<String>>("track-features");
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
            extras,
            flags,
            channel,
            subdir,
            license,
            license_family,
            when,
            track_features,
        })
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlWhen {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(str) => {
                // Validate eagerly so the error carries the TOML span.
                if let Err(err) = parse_when_matchspec(&str) {
                    return Err(DeserError::from(err.into_toml_error(value.span)));
                }
                Ok(TomlWhen::MatchSpec(str.to_string()))
            }
            ValueInner::Array(_) => Err(DeserError::from(toml_span::Error {
                kind: ErrorKind::Custom(
                    "`when` must be a string or a table with `all`, `any`, or `package`; top-level arrays are not allowed".into(),
                ),
                span: value.span,
                line_info: None,
            })),
            inner @ ValueInner::Table(_) => {
                let mut table_value = Value::with_span(inner, value.span);

                // Decide between all / any / package by inspecting which keys
                // the table contains.
                let (has_all, has_any, has_package) =
                    if let ValueInner::Table(table) = table_value.as_ref() {
                        (
                            table.contains_key("all"),
                            table.contains_key("any"),
                            table.contains_key("package"),
                        )
                    } else {
                        unreachable!("table_value was just constructed from a Table")
                    };

                match (has_all, has_any, has_package) {
                    (true, false, false) => {
                        let mut th = TableHelper::new(&mut table_value)?;
                        let all = th.required_s::<Vec<TomlWhen>>("all")?;
                        if all.value.is_empty() {
                            return Err(DeserError::from(toml_span::Error {
                                kind: ErrorKind::Custom(
                                    "`when.all` must contain at least one condition".into(),
                                ),
                                span: all.span,
                                line_info: None,
                            }));
                        }
                        th.finalize(None)?;
                        Ok(TomlWhen::All { all: all.value })
                    }
                    (false, true, false) => {
                        let mut th = TableHelper::new(&mut table_value)?;
                        let any = th.required_s::<Vec<TomlWhen>>("any")?;
                        if any.value.is_empty() {
                            return Err(DeserError::from(toml_span::Error {
                                kind: ErrorKind::Custom(
                                    "`when.any` must contain at least one condition".into(),
                                ),
                                span: any.span,
                                line_info: None,
                            }));
                        }
                        th.finalize(None)?;
                        Ok(TomlWhen::Any { any: any.value })
                    }
                    (false, false, true) => {
                        // Take `package` out and hand the remainder to
                        // `TomlSpec::deserialize` so all matchspec fields
                        // (`subdir`, `channel`, `extras`, ...) are accepted.
                        let mut th = TableHelper::new(&mut table_value)?;
                        let package = th
                            .required::<TomlFromStr<PackageName>>("package")?
                            .into_inner();
                        th.finalize(Some(&mut table_value))?;

                        let spec =
                            <TomlSpec as toml_span::Deserialize>::deserialize(&mut table_value)?;
                        validate_when_spec(&spec, table_value.span)?;
                        Ok(TomlWhen::Expanded(TomlWhenPackage {
                            package,
                            spec: Box::new(spec),
                        }))
                    }
                    _ => Err(DeserError::from(toml_span::Error {
                        kind: ErrorKind::Custom(
                            "`when` tables must contain exactly one of `all`, `any`, or `package`"
                                .into(),
                        ),
                        span: table_value.span,
                        line_info: None,
                    })),
                }
            }
            inner => Err(expected("a string or a table", inner, value.span).into()),
        }
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
                    .map(PixiSpec::from)
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
                        .map_err(|e| DeserError::from(e.into_toml_error(table_value.span)))?,
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
                    .map(PixiSpec::from)
                    .map_err(serde_untagged::de::Error::custom)
            })
            .map(|map| {
                let spec: TomlSpec = map.deserialize()?;
                spec.into_spec()
                    .map_err(|err| serde_untagged::de::Error::custom(err.message_with_help()))
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
    use insta::assert_snapshot;
    use pixi_test_utils::format_parse_error;
    use pixi_toml::TomlDiagnostic;
    use serde::Serialize;
    use serde_json::{Value, json};

    use super::*;

    fn parse_toml_condition(input: &str) -> String {
        let mut value = toml_span::parse(input).expect("valid TOML");
        let spec = <PixiSpec as toml_span::Deserialize>::deserialize(&mut value)
            .expect("expected parse to succeed");
        spec.as_detailed()
            .expect("`when` only accepted on detailed specs")
            .condition
            .as_ref()
            .expect("expected parsed `when` condition")
            .to_string()
    }

    fn parse_json_condition(input: Value) -> String {
        let spec: PixiSpec = serde_json::from_value(input).expect("expected parse to succeed");
        spec.as_detailed()
            .expect("`when` only accepted on detailed specs")
            .condition
            .as_ref()
            .expect("expected parsed `when` condition")
            .to_string()
    }

    /// Parse a TOML pixi spec and render the first error via miette so the
    /// snapshot shows the offending source span.
    fn parse_toml_error(input: &str) -> String {
        let trimmed = input.trim();
        let result = toml_span::parse(trimmed)
            .map_err(DeserError::from)
            .and_then(|mut v| <PixiSpec as toml_span::Deserialize>::deserialize(&mut v));
        let first = result
            .expect_err("expected a parse failure")
            .errors
            .into_iter()
            .next()
            .expect("DeserError contained no errors");
        format_parse_error(trimmed, TomlDiagnostic(first))
    }

    /// Parse a TOML pixi spec successfully.
    fn parse_toml_ok(input: &str) -> PixiSpec {
        let mut value = toml_span::parse(input.trim()).expect("expected valid TOML");
        <PixiSpec as toml_span::Deserialize>::deserialize(&mut value)
            .expect("expected parse to succeed")
    }

    /// Parse a JSON pixi spec successfully.
    fn parse_json_ok(input: Value) -> PixiSpec {
        serde_json::from_value::<PixiSpec>(input).expect("expected parse to succeed")
    }

    /// JSON parse failures don't carry source spans, so we just stringify.
    fn parse_json_error(input: Value) -> String {
        serde_json::from_value::<PixiSpec>(input)
            .expect_err("expected a parse failure")
            .to_string()
    }

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
            // Source specs with matchspec selectors:
            json!({ "path": "../mypkg", "version": "1.2.3" }),
            json!({ "path": "../mypkg", "version": ">=1.2", "build": "py37_*" }),
            json!({ "path": "../mypkg", "subdir": "linux-64", "track-features": ["legacy"] }),
            json!({ "git": "https://github.com/foo/bar", "branch": "main", "version": ">=1.0", "extras": ["cuda"] }),
            json!({ "url": "https://example.com/foo.tar.gz", "version": "1.2.3", "build-number": ">=3" }),
            json!({ "url": "https://example.com/foo.tar.gz", "flags": ["cuda", "blas:*"] }),
            // Errors:
            json!({ "ver": "1.2.3" }),
            json!({ "version": "//" }),
            json!({ "path": "foobar", "version": "//" }),
            json!({ "path": "foobar", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "branch": "main", "tag": "v1" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            // A `.conda` URL is binary; matchspec selectors must be rejected.
            json!({ "url": "https://example.com/foo.conda", "version": "1.2.3" }),
            // A `.conda` path is binary; matchspec selectors must be rejected.
            json!({ "path": "foo.conda", "version": "1.2.3" }),
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

    #[test]
    fn test_v3_detailed_fields() {
        let spec: PixiSpec = serde_json::from_value(json!({
            "version": ">=1.0",
            "extras": ["cuda"],
            "flags": ["cuda", "blas:*"],
            "license-family": "BSD",
            "track-features": ["legacy"],
        }))
        .unwrap();
        let detailed = spec.as_detailed().unwrap();
        assert_eq!(detailed.extras, Some(vec!["cuda".to_string()]));
        assert_eq!(
            detailed
                .flags
                .as_ref()
                .unwrap()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["cuda".to_string(), "blas:*".to_string()]
        );
        assert_eq!(detailed.license_family.as_deref(), Some("BSD"));
        assert_eq!(detailed.track_features, Some(vec!["legacy".to_string()]));

        let err = serde_json::from_value::<PixiSpec>(json!("1.2.3[flags=[cuda]]")).unwrap_err();
        assert!(err.to_string().contains("flags"));
    }

    #[test]
    fn test_when_condition_syntax() {
        use rattler_conda_types::MatchSpecCondition;

        let spec: PixiSpec = serde_json::from_value(json!({
            "version": "*",
            "when": "__unix"
        }))
        .unwrap();
        assert_eq!(
            spec.as_detailed()
                .unwrap()
                .condition
                .as_ref()
                .unwrap()
                .to_string(),
            "__unix"
        );

        let spec: PixiSpec = serde_json::from_value(json!({
            "version": "*",
            "when": { "package": "python", "version": ">=3.10", "build": "*cuda" }
        }))
        .unwrap();
        assert_eq!(
            spec.as_detailed()
                .unwrap()
                .condition
                .as_ref()
                .unwrap()
                .to_string(),
            "python[version=\">=3.10\", build=\"*cuda\"]"
        );

        let spec: PixiSpec = serde_json::from_value(json!({
            "version": "*",
            "when": {
                "all": [
                    "__unix",
                    "python >=3.10",
                    { "package": "numpy", "version": ">=2", "build": "*cuda" }
                ]
            }
        }))
        .unwrap();

        let condition = spec.as_detailed().unwrap().condition.as_ref().unwrap();
        let MatchSpecCondition::And(left, right) = condition else {
            panic!("expected top-level AND condition");
        };
        let MatchSpecCondition::And(left_left, left_right) = left.as_ref() else {
            panic!("expected nested AND condition");
        };
        assert_eq!(left_left.to_string(), "__unix");
        assert_eq!(left_right.to_string(), "python>=3.10");
        assert_eq!(right.to_string(), "numpy[version=\">=2\", build=\"*cuda\"]");

        let spec: PixiSpec = serde_json::from_value(json!({
            "version": "*",
            "when": { "any": ["__linux", "__osx"] }
        }))
        .unwrap();
        assert_eq!(
            spec.as_detailed()
                .unwrap()
                .condition
                .as_ref()
                .unwrap()
                .to_string(),
            "(__linux or __osx)"
        );

        let spec: PixiSpec = serde_json::from_value(json!({
            "version": "*",
            "when": { "all": ["__unix", { "any": ["__linux", "__osx"] }] }
        }))
        .unwrap();
        assert_eq!(
            spec.as_detailed()
                .unwrap()
                .condition
                .as_ref()
                .unwrap()
                .to_string(),
            "(__unix and (__linux or __osx))"
        );

        let mut value = toml_span::parse(
            r#"
            version = "*"
            when = { all = ["__unix", { package = "python", version = ">=3.10", build = "*cuda" }] }
            "#,
        )
        .unwrap();
        let spec = <PixiSpec as toml_span::Deserialize>::deserialize(&mut value).unwrap();
        assert_eq!(
            spec.as_detailed()
                .unwrap()
                .condition
                .as_ref()
                .unwrap()
                .to_string(),
            "(__unix and python[version=\">=3.10\", build=\"*cuda\"])"
        );
    }

    #[test]
    fn test_when_condition_string_matchspec() {
        let input = json!({ "version": "*", "when": "__unix" });
        assert_snapshot!(parse_json_condition(input), @"__unix");
    }

    #[test]
    fn test_when_condition_all() {
        let input = json!({ "version": "*", "when": { "all": ["__unix", "python >=3.10"] } });
        assert_snapshot!(parse_json_condition(input), @"(__unix and python>=3.10)");
    }

    #[test]
    fn test_when_condition_any() {
        let input = json!({ "version": "*", "when": { "any": ["__linux", "__osx"] } });
        assert_snapshot!(parse_json_condition(input), @"(__linux or __osx)");
    }

    #[test]
    fn test_when_condition_nested_all_any() {
        let input = json!({
            "version": "*",
            "when": { "all": ["__unix", { "any": ["__linux", "__osx"] }] },
        });
        assert_snapshot!(parse_json_condition(input), @"(__unix and (__linux or __osx))");
    }

    #[test]
    fn test_when_condition_expanded_build_match() {
        let input = json!({
            "version": "*",
            "when": { "package": "python", "version": ">=3.10", "build": "*cuda" },
        });
        assert_snapshot!(parse_json_condition(input), @r###"python[version=">=3.10", build="*cuda"]"###);
    }

    #[test]
    fn test_when_condition_expanded_with_build_number() {
        let input = json!({
            "version": "*",
            "when": { "package": "python", "version": ">=3.10", "build-number": ">=3" },
        });
        assert_snapshot!(parse_json_condition(input), @r###"python[version=">=3.10", build_number=">=3"]"###);
    }

    #[test]
    fn test_when_condition_expanded_with_subdir() {
        let input = json!({
            "version": "*",
            "when": { "package": "python", "subdir": "linux-64" },
        });
        assert_snapshot!(parse_json_condition(input), @r###"python[subdir="linux-64"]"###);
    }

    #[test]
    fn test_when_condition_expanded_with_extras() {
        let input = json!({
            "version": "*",
            "when": { "package": "python", "extras": ["dev"] },
        });
        assert_snapshot!(parse_json_condition(input), @"python[extras=[dev]]");
    }

    #[test]
    fn test_when_condition_expanded_with_flags() {
        let input = json!({
            "version": "*",
            "when": { "package": "python", "flags": ["cuda", "blas:*"] },
        });
        assert_snapshot!(parse_json_condition(input), @"python[flags=[cuda, blas:*]]");
    }

    #[test]
    fn test_when_condition_expanded_with_track_features() {
        let input = json!({
            "version": "*",
            "when": { "package": "python", "track-features": ["legacy"] },
        });
        assert_snapshot!(parse_json_condition(input), @r###"python[track_features="legacy"]"###);
    }

    #[test]
    fn test_when_condition_expanded_with_file_name() {
        let input = json!({
            "version": "*",
            "when": { "package": "python", "file-name": "python-3.10.0-h12debd9_0.tar.bz2" },
        });
        assert_snapshot!(parse_json_condition(input), @r###"python[fn="python-3.10.0-h12debd9_0.tar.bz2"]"###);
    }

    #[test]
    fn test_when_condition_expanded_with_license() {
        let input = json!({
            "version": "*",
            "when": { "package": "python", "license": "MIT", "license-family": "BSD" },
        });
        assert_snapshot!(parse_json_condition(input), @r###"python[license="MIT", license_family="BSD"]"###);
    }

    #[test]
    fn test_when_condition_toml_all_with_expanded_build_match() {
        let input = r#"version = "*"
when = { all = ["__unix", { package = "python", version = ">=3.10", build = "*cuda" }] }"#;
        assert_snapshot!(parse_toml_condition(input), @r###"(__unix and python[version=">=3.10", build="*cuda"])"###);
    }

    #[test]
    fn test_when_condition_toml_any() {
        let input = r#"version = "*"
when = { any = ["__linux", "__osx"] }"#;
        assert_snapshot!(parse_toml_condition(input), @"(__linux or __osx)");
    }

    #[test]
    fn test_when_condition_toml_expanded_with_build_number() {
        let input = r#"version = "*"
when = { package = "python", version = ">=3.10", build-number = ">=3" }"#;
        assert_snapshot!(parse_toml_condition(input), @r###"python[version=">=3.10", build_number=">=3"]"###);
    }

    #[test]
    fn test_when_condition_toml_expanded_with_extras_and_flags() {
        let input = r#"version = "*"
when = { package = "python", extras = ["dev"], flags = ["cuda", "blas:*"] }"#;
        assert_snapshot!(parse_toml_condition(input), @"python[extras=[dev], flags=[cuda, blas:*]]");
    }

    #[test]
    fn test_when_condition_toml_expanded_with_track_features() {
        let input = r#"version = "*"
when = { package = "python", track-features = ["legacy"] }"#;
        assert_snapshot!(parse_toml_condition(input), @r###"python[track_features="legacy"]"###);
    }

    #[test]
    fn test_when_rejects_unsupported_shorthand() {
        serde_json::from_value::<PixiSpec>(
            json!({ "version": "*", "when": ["__unix", "python >=3.10"] }),
        )
        .unwrap_err();

        for input in [
            json!({ "version": "*", "when": "python[version='>=3.10']" }),
            json!({ "version": "*", "when": "python >=3.10 *cuda" }),
        ] {
            let err = serde_json::from_value::<PixiSpec>(input).unwrap_err();
            let err = err.to_string();
            assert!(err.contains("when"), "expected `when` error, got: {err}");
        }

        for input in [
            r#"version = "*"
               when = ["__unix"]"#,
            r#"version = "*"
               when = { all = ["__unix"], any = ["__linux"] }"#,
            r#"version = "*"
               when = { all = [] }"#,
            r#"version = "*"
               when = { any = [] }"#,
        ] {
            let mut value = toml_span::parse(input).unwrap();
            let err = <PixiSpec as toml_span::Deserialize>::deserialize(&mut value)
                .unwrap_err()
                .to_string();
            assert!(err.contains("when"), "expected `when` error, got: {err}");
        }
    }

    #[test]
    fn test_when_reject_top_level_array_json() {
        let input = json!({ "version": "*", "when": ["__unix", "python >=3.10"] });
        assert_snapshot!(parse_json_error(input), @"`when` must be a string or a table with `all`, `any`, or `package`; top-level arrays are not allowed");
    }

    #[test]
    fn test_when_reject_bracket_matchspec_json() {
        let input = json!({ "version": "*", "when": "python[version='>=3.10']" });
        assert_snapshot!(parse_json_error(input), @r###"`when` strings do not support bracket matchspec syntax; use the expanded form `when = { package = "python", version = ">=3.10" }`"###);
    }

    #[test]
    fn test_when_reject_bracket_matchspec_expands_multiple_fields_json() {
        let input = json!({
            "version": "*",
            "when": "python[version='>=3.10',build_number='>=3',subdir='linux-64']",
        });
        assert_snapshot!(parse_json_error(input), @r###"`when` strings do not support bracket matchspec syntax; use the expanded form `when = { package = "python", version = ">=3.10", build-number = ">=3", subdir = "linux-64" }`"###);
    }

    #[test]
    fn test_when_reject_bracket_matchspec_expands_metadata_fields_json() {
        let input = json!({
            "version": "*",
            "when": "python[version='>=3.10',fn='python-3.10.0-h123_0.conda',license='BSD-3-Clause',license_family='BSD']",
        });
        assert_snapshot!(parse_json_error(input), @r###"`when` strings do not support bracket matchspec syntax; use the expanded form `when = { package = "python", version = ">=3.10", file-name = "python-3.10.0-h123_0.conda", license = "BSD-3-Clause", license-family = "BSD" }`"###);
    }

    #[test]
    fn test_when_reject_build_shorthand_json() {
        let input = json!({ "version": "*", "when": "python >=3.10 *cuda" });
        assert_snapshot!(parse_json_error(input), @r###"`when` strings do not support build-string shorthand; use the expanded form `when = { package = "python", version = ">=3.10", build = "*cuda" }`"###);
    }

    #[test]
    fn test_when_reject_build_shorthand_expands_name_version_build_json() {
        let input = json!({ "version": "*", "when": "foobar >=123.3 *cuda" });
        assert_snapshot!(parse_json_error(input), @r###"`when` strings do not support build-string shorthand; use the expanded form `when = { package = "foobar", version = ">=123.3", build = "*cuda" }`"###);
    }

    #[test]
    fn test_when_reject_build_shorthand_expands_channel_json() {
        let input = json!({ "version": "*", "when": "conda-forge::python >=3.10 *cuda" });
        assert_snapshot!(parse_json_error(input), @r###"`when` strings do not support build-string shorthand; use the expanded form `when = { package = "python", version = ">=3.10", build = "*cuda", channel = "conda-forge" }`"###);
    }

    #[test]
    fn test_when_reject_build_shorthand_expands_channel_subdir_json() {
        let input = json!({ "version": "*", "when": "conda-forge/linux-64::python >=3.10 py310*" });
        assert_snapshot!(parse_json_error(input), @r###"`when` strings do not support build-string shorthand; use the expanded form `when = { package = "python", version = ">=3.10", build = "py310*", channel = "conda-forge", subdir = "linux-64" }`"###);
    }

    #[test]
    fn test_when_reject_string_no_package_name_json() {
        let input = json!({ "version": "*", "when": ">=3.10" });
        assert_snapshot!(parse_json_error(input), @"invalid `when` matchspec: missing package name");
    }

    #[test]
    fn test_when_reject_string_with_channel_prefix_json() {
        let input = json!({ "version": "*", "when": "conda-forge::python >=3.10" });
        assert_snapshot!(parse_json_error(input), @r###"`when` strings do not support channel prefixes; this would be `when = { package = "python", version = ">=3.10", channel = "conda-forge" }`, but `channel` is not supported inside `when` tables"###);
    }

    #[test]
    fn test_when_reject_expanded_with_url_json() {
        let input = json!({
            "version": "*",
            "when": { "package": "python", "url": "https://example.com/python-3.10.conda" },
        });
        assert_snapshot!(parse_json_error(input), @"source-location fields (`url`, `git`, `path`, `md5`, `sha256`, ...) are not allowed inside `when` tables");
    }

    #[test]
    fn test_when_reject_expanded_with_channel_json() {
        let input = json!({
            "version": "*",
            "when": { "package": "python", "channel": "conda-forge" },
        });
        assert_snapshot!(parse_json_error(input), @"`channel` is not supported inside `when` tables");
    }

    #[test]
    fn test_when_accept_combined_with_path_json() {
        // Matchspec fields including `when` are allowed alongside a path
        // source (a directory or non-binary archive).
        let input = json!({ "path": "../foo", "when": "__unix" });
        let spec = parse_json_ok(input);
        let path = spec.as_path_source().expect("expected a path source spec");
        assert!(path.matchspec.condition.is_some());
    }

    #[test]
    fn test_matchspec_reject_with_binary_path_json() {
        // A path pointing at a `.conda` archive is binary; matchspec
        // selectors are meaningless there and must be rejected.
        let input = json!({ "path": "foo.conda", "when": "__unix" });
        assert_snapshot!(parse_json_error(input), @"`when` cannot be used with `path`");
    }

    #[test]
    fn test_when_reject_top_level_array_toml() {
        let input = r#"version = "*"
when = ["__unix"]"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × `when` must be a string or a table with `all`, `any`, or `package`; top-level arrays are not allowed
          ╭─[pixi.toml:2:8]
        1 │ version = "*"
        2 │ when = ["__unix"]
          ·        ──────────
          ╰────
        "###);
    }

    #[test]
    fn test_when_reject_all_and_any_toml() {
        let input = r#"version = "*"
when = { all = ["__unix"], any = ["__linux"] }"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × `when` tables must contain exactly one of `all`, `any`, or `package`
          ╭─[pixi.toml:2:8]
        1 │ version = "*"
        2 │ when = { all = ["__unix"], any = ["__linux"] }
          ·        ───────────────────────────────────────
          ╰────
        "###);
    }

    #[test]
    fn test_when_reject_empty_all_toml() {
        let input = r#"version = "*"
when = { all = [] }"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × `when.all` must contain at least one condition
          ╭─[pixi.toml:2:16]
        1 │ version = "*"
        2 │ when = { all = [] }
          ·                ──
          ╰────
        "###);
    }

    #[test]
    fn test_when_reject_empty_any_toml() {
        let input = r#"version = "*"
when = { any = [] }"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × `when.any` must contain at least one condition
          ╭─[pixi.toml:2:16]
        1 │ version = "*"
        2 │ when = { any = [] }
          ·                ──
          ╰────
        "###);
    }

    #[test]
    fn test_when_reject_string_no_package_name_toml() {
        let input = r#"version = "*"
when = ">=3.10""#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × invalid `when` matchspec: missing package name
          ╭─[pixi.toml:2:9]
        1 │ version = "*"
        2 │ when = ">=3.10"
          ·         ──────
          ╰────
        "###);
    }

    #[test]
    fn test_when_reject_string_with_channel_prefix_toml() {
        let input = r#"version = "*"
when = "conda-forge::python >=3.10""#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × `when` strings do not support channel prefixes
          ╭─[pixi.toml:2:9]
        1 │ version = "*"
        2 │ when = "conda-forge::python >=3.10"
          ·         ──────────────────────────
          ╰────
         help: this would be `when = { package = "python", version = ">=3.10", channel = "conda-forge" }`, but `channel` is not supported inside `when` tables
        "###);
    }

    #[test]
    fn test_when_reject_expanded_with_url_toml() {
        let input = r#"version = "*"
when = { package = "python", url = "https://example.com/python-3.10.conda" }"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × source-location fields (`url`, `git`, `path`, `md5`, `sha256`, ...) are not allowed inside `when` tables
          ╭─[pixi.toml:2:8]
        1 │ version = "*"
        2 │ when = { package = "python", url = "https://example.com/python-3.10.conda" }
          ·        ─────────────────────────────────────────────────────────────────────
          ╰────
        "###);
    }

    #[test]
    fn test_when_reject_expanded_with_path_toml() {
        let input = r#"version = "*"
when = { package = "python", path = "../foo" }"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × source-location fields (`url`, `git`, `path`, `md5`, `sha256`, ...) are not allowed inside `when` tables
          ╭─[pixi.toml:2:8]
        1 │ version = "*"
        2 │ when = { package = "python", path = "../foo" }
          ·        ───────────────────────────────────────
          ╰────
        "###);
    }

    #[test]
    fn test_when_reject_expanded_with_md5_toml() {
        let input = r#"version = "*"
when = { package = "python", md5 = "d41d8cd98f00b204e9800998ecf8427e" }"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × source-location fields (`url`, `git`, `path`, `md5`, `sha256`, ...) are not allowed inside `when` tables
          ╭─[pixi.toml:2:8]
        1 │ version = "*"
        2 │ when = { package = "python", md5 = "d41d8cd98f00b204e9800998ecf8427e" }
          ·        ────────────────────────────────────────────────────────────────
          ╰────
        "###);
    }

    #[test]
    fn test_when_reject_expanded_with_branch_toml() {
        let input = r#"version = "*"
when = { package = "python", branch = "main" }"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × source-location fields (`url`, `git`, `path`, `md5`, `sha256`, ...) are not allowed inside `when` tables
          ╭─[pixi.toml:2:8]
        1 │ version = "*"
        2 │ when = { package = "python", branch = "main" }
          ·        ───────────────────────────────────────
          ╰────
        "###);
    }

    #[test]
    fn test_when_reject_expanded_with_channel_toml() {
        let input = r#"version = "*"
when = { package = "python", channel = "conda-forge" }"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × `channel` is not supported inside `when` tables
          ╭─[pixi.toml:2:8]
        1 │ version = "*"
        2 │ when = { package = "python", channel = "conda-forge" }
          ·        ───────────────────────────────────────────────
          ╰────
        "###);
    }

    #[test]
    fn test_when_reject_table_without_directive_toml() {
        let input = r#"version = "*"
when = { build = "*cuda" }"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × `when` tables must contain exactly one of `all`, `any`, or `package`
          ╭─[pixi.toml:2:8]
        1 │ version = "*"
        2 │ when = { build = "*cuda" }
          ·        ───────────────────
          ╰────
        "###);
    }

    #[test]
    fn test_when_reject_expanded_invalid_version_toml() {
        let input = r#"version = "*"
when = { package = "python", version = "//" }"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × it seems you're trying to add a path dependency, please specify as a table with a `path` key: '{ path = "//" }'
          ╭─[pixi.toml:2:41]
        1 │ version = "*"
        2 │ when = { package = "python", version = "//" }
          ·                                         ──
          ╰────
        "###);
    }

    #[test]
    fn test_when_reject_nested_invalid_leaf_toml() {
        let input = r#"version = "*"
when = { all = ["__unix", "python[version='>=3.10']"] }"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × `when` strings do not support bracket matchspec syntax
          ╭─[pixi.toml:2:28]
        1 │ version = "*"
        2 │ when = { all = ["__unix", "python[version='>=3.10']"] }
          ·                            ────────────────────────
          ╰────
         help: use the expanded form `when = { package = "python", version = ">=3.10" }`
        "###);
    }

    #[test]
    fn test_when_accept_combined_with_path_toml() {
        // A `path = ../foo` is a non-binary source location, so matchspec
        // selectors including `when` are accepted.
        let input = r#"path = "../foo"
when = "__unix""#;
        let spec = parse_toml_ok(input);
        let path = spec.as_path_source().expect("expected a path source spec");
        assert!(path.matchspec.condition.is_some());
    }

    #[test]
    fn test_v3_reject_extras_not_array_json() {
        let input = json!({ "version": "*", "extras": "dev" });
        assert_snapshot!(parse_json_error(input), @r###"invalid type: string "dev", expected a sequence"###);
    }

    #[test]
    fn test_v3_reject_extras_non_string_element_json() {
        let input = json!({ "version": "*", "extras": [42] });
        assert_snapshot!(parse_json_error(input), @"invalid type: integer `42`, expected a string");
    }

    #[test]
    fn test_v3_reject_flags_not_array_json() {
        let input = json!({ "version": "*", "flags": "cuda" });
        assert_snapshot!(parse_json_error(input), @r###"invalid type: string "cuda", expected a sequence"###);
    }

    #[test]
    fn test_v3_accept_extras_with_path_json() {
        let input = json!({ "path": "../foo", "extras": ["dev"] });
        let spec = parse_json_ok(input);
        let path = spec.as_path_source().expect("expected a path source spec");
        assert_eq!(
            path.matchspec.extras.as_deref(),
            Some(&["dev".to_string()][..])
        );
    }

    #[test]
    fn test_v3_accept_flags_with_git_json() {
        let input = json!({ "git": "https://example.com/foo.git", "flags": ["cuda"] });
        let spec = parse_json_ok(input);
        let git = spec.as_git().expect("expected a git spec");
        assert!(git.matchspec.flags.is_some());
    }

    #[test]
    fn test_v3_reject_track_features_with_url_json() {
        // `.conda` extension makes this a binary URL; matchspec selectors
        // are rejected.
        let input = json!({
            "url": "https://example.com/foo.conda",
            "track-features": ["legacy"],
        });
        assert_snapshot!(parse_json_error(input), @"`track-features` cannot be used with `url`");
    }

    #[test]
    fn test_v3_accept_track_features_with_source_url_json() {
        // Non-binary URL extension: matchspec selectors are accepted.
        let input = json!({
            "url": "https://example.com/foo.tar.gz",
            "track-features": ["legacy"],
        });
        let spec = parse_json_ok(input);
        let url = spec.as_url_source().expect("expected a url source spec");
        assert_eq!(
            url.matchspec.track_features.as_deref(),
            Some(&["legacy".to_string()][..])
        );
    }

    #[test]
    fn test_v3_reject_extras_not_array_toml() {
        let input = r#"version = "*"
extras = "dev""#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × expected an array, found string
          ╭─[pixi.toml:2:11]
        1 │ version = "*"
        2 │ extras = "dev"
          ·           ───
          ╰────
        "###);
    }

    #[test]
    fn test_v3_reject_flags_not_array_toml() {
        let input = r#"version = "*"
flags = "cuda""#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × expected array, found string
          ╭─[pixi.toml:2:10]
        1 │ version = "*"
        2 │ flags = "cuda"
          ·          ────
          ╰────
        "###);
    }

    #[test]
    fn test_v3_reject_flags_invalid_regex_toml() {
        let input = r#"version = "*"
flags = ["^[invalid$"]"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × invalid regex: ^[invalid$
          ╭─[pixi.toml:2:11]
        1 │ version = "*"
        2 │ flags = ["^[invalid$"]
          ·           ──────────
          ╰────
        "###);
    }

    #[test]
    fn test_v3_reject_flags_invalid_glob_toml() {
        let input = r#"version = "*"
flags = ["*[unclosed"]"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × invalid glob: *[unclosed
          ╭─[pixi.toml:2:11]
        1 │ version = "*"
        2 │ flags = ["*[unclosed"]
          ·           ──────────
          ╰────
        "###);
    }

    #[test]
    fn test_v3_accept_extras_with_path_toml() {
        let input = r#"path = "../foo"
extras = ["dev"]"#;
        let spec = parse_toml_ok(input);
        let path = spec.as_path_source().expect("expected a path source spec");
        assert_eq!(
            path.matchspec.extras.as_deref(),
            Some(&["dev".to_string()][..])
        );
    }

    #[test]
    fn test_v3_reject_flags_with_binary_url_toml() {
        // `.conda` extension marks this as a binary URL: matchspec rejected.
        let input = r#"url = "https://example.com/foo.conda"
flags = ["cuda"]"#;
        assert_snapshot!(parse_toml_error(input), @r###"
         × `flags` cannot be used with `url`
          ╭─[pixi.toml:1:1]
        1 │ ╭─▶ url = "https://example.com/foo.conda"
        2 │ ╰─▶ flags = ["cuda"]
          ╰────
        "###);
    }

    #[test]
    fn test_v3_accept_flags_with_source_url_toml() {
        // `.tar.gz` is not a binary conda archive: matchspec accepted.
        let input = r#"url = "https://example.com/foo.tar.gz"
flags = ["cuda"]"#;
        let spec = parse_toml_ok(input);
        let url = spec.as_url_source().expect("expected a url source spec");
        assert!(url.matchspec.flags.is_some());
    }

    #[test]
    fn test_v3_accept_track_features_with_git_toml() {
        let input = r#"git = "https://example.com/foo.git"
track-features = ["legacy"]"#;
        let spec = parse_toml_ok(input);
        let git = spec.as_git().expect("expected a git spec");
        assert_eq!(
            git.matchspec.track_features.as_deref(),
            Some(&["legacy".to_string()][..])
        );
    }

    /// Source-side matchspec round-trips through JSON serialization:
    /// a spec mixing a source location with several matchspec selectors
    /// must parse, serialize, and re-parse to an equal `PixiSpec`.
    #[test]
    fn test_source_matchspec_roundtrip_full() {
        let inputs: Vec<Value> = vec![
            // path + multiple matchspec fields
            json!({
                "path": "../my-pkg",
                "version": "1.2.3",
                "build": "py37_*",
                "build-number": ">=3",
                "extras": ["cuda", "mkl"],
                "subdir": "linux-64",
                "license": "BSD-3-Clause",
                "track-features": ["legacy"],
            }),
            // git + matchspec
            json!({
                "git": "https://github.com/foo/bar",
                "branch": "main",
                "version": ">=1.0",
                "extras": ["dev"],
            }),
            // non-binary url + matchspec
            json!({
                "url": "https://example.com/foo.tar.gz",
                "version": "1.2.3",
                "build-number": ">=3",
                "flags": ["cuda"],
            }),
        ];

        for input in inputs {
            let first: PixiSpec =
                serde_json::from_value(input.clone()).expect("expected initial parse to succeed");
            let serialized = serde_json::to_value(&first).expect("serialize should succeed");
            let second: PixiSpec = serde_json::from_value(serialized.clone())
                .expect("round-trip parse should succeed");
            assert_eq!(
                first, second,
                "round-trip mismatch for input:\n{input}\n\nrendered as: {serialized}"
            );
        }
    }

    /// A path source carrying matchspec selectors round-trips through
    /// `PixiSpec::try_into_source_spec` and back into a `PixiSpec`, and
    /// the matchspec fields survive intact via the accessor.
    #[test]
    fn test_source_location_spec_matchspec_accessor() {
        use crate::SourceLocationSpec;

        let spec = parse_toml_ok(
            r#"path = "../my-pkg"
version = ">=2.0"
build = "py310_*"
extras = ["test"]"#,
        );

        // Routing into a SourceLocationSpec preserves matchspec.
        let source = spec
            .clone()
            .try_into_source_spec()
            .expect("expected a source spec");
        assert!(matches!(source, SourceLocationSpec::Path(_)));
        let matchspec = source.matchspec();
        assert_eq!(matchspec.version.as_ref().unwrap().to_string(), ">=2.0");
        assert!(matchspec.build.is_some());
        assert_eq!(matchspec.extras.as_deref(), Some(&["test".to_string()][..]));

        // Lifting it back through `From<SourceLocationSpec>` returns the
        // same `PixiSpec`.
        let round_tripped = PixiSpec::from(source);
        assert_eq!(spec, round_tripped);
    }

    /// `try_into_source_spec` returns the original `PixiSpec` (via `Err`)
    /// when invoked on a binary variant.
    #[test]
    fn test_try_into_source_spec_rejects_binary() {
        let detailed: PixiSpec = parse_json_ok(json!({ "version": "1.2.3" }));
        let err = detailed.try_into_source_spec().unwrap_err();
        assert!(matches!(err, PixiSpec::Detailed(_)));

        let url_binary = parse_json_ok(json!({
            "url": "https://conda.anaconda.org/conda-forge/linux-64/foo-1.0-py.conda",
        }));
        let err = url_binary.try_into_source_spec().unwrap_err();
        assert!(matches!(err, PixiSpec::UrlBinary(_)));
    }
}
