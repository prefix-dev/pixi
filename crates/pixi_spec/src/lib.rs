#![deny(missing_docs)]

//! This crate defines the [`PixiSpec`] type which represents a package
//! specification for pixi.
//!
//! The `PixiSpec` type represents the user input for a package. It can
//! represent both source and binary packages. The `PixiSpec` type can
//! optionally be converted to a `NamelessMatchSpec` which is used to match
//! binary packages.

mod detailed;
mod dev_source;
mod exclude_newer;
mod git;
mod matchspec_fields;
mod path;
mod pin;
mod source_anchor;
mod subdirectory;
mod toml;
mod url;

use std::{fmt::Display, path::PathBuf, str::FromStr};

pub use detailed::DetailedSpec;
pub use dev_source::DevSourceSpec;
pub use exclude_newer::{ExcludeNewer, IndexExcludeNewer, ResolvedExcludeNewer};
pub use git::{GitLocationSpec, GitReference, GitReferenceError, GitSpec};
use itertools::Either;
pub use matchspec_fields::MatchspecFields;
pub use path::{PathBinarySpec, PathSourceSpec, PathSpec};
pub use pin::{Pin, PinBound, PinError, PinExpression};
use rattler_conda_types::{
    ChannelConfig, MatchSpec, NamedChannelOrUrl, NamelessMatchSpec, PackageName, ParseChannelError,
    VersionSpec, package::CondaArchiveType,
};
#[cfg(feature = "rattler_lock")]
pub use rattler_lock::Verbatim;
pub use source_anchor::SourceAnchor;
pub use subdirectory::{Subdirectory, SubdirectoryError};
use thiserror::Error;
pub use toml::{TomlLocationSpec, TomlSpec, TomlVersionSpecStr};
use url::url_is_binary;
pub use url::{UrlBinarySpec, UrlSourceSpec, UrlSpec};

/// An error that is returned when a spec cannot be converted into another spec
/// type.
#[derive(Debug, Error)]
pub enum SpecConversionError {
    /// The root directory is not an absolute path
    #[error("root directory from channel config is not an absolute path")]
    NonAbsoluteRootDir(PathBuf),

    /// The root directory is not UTF-8 encoded.
    #[error("root directory of channel config is not utf8 encoded")]
    NotUtf8RootDir(PathBuf),

    /// Encountered an invalid path
    #[error("invalid path '{0}'")]
    InvalidPath(String),

    /// Encountered an invalid channel url or path
    #[error("the channel '{0}' could not be resolved")]
    InvalidChannel(String, #[source] ParseChannelError),

    /// The `name` field is missing in the spec.
    #[error("the `package.name` must be provided in versions of pixi-build-api-version <2")]
    MissingName,

    /// A wildcard platform glob was used in a package target selector.
    #[error("wildcard target selector '{0}' is not supported in package targets")]
    WildcardTargetSelector(String),
}

/// A package specification for pixi.
///
/// This type can represent both source and binary packages. Use the
/// [`Self::try_into_nameless_match_spec`] method to convert this type into a
/// type that only represents binary packages.
///
/// The variants encode the binary-vs-source distinction at the type level
/// rather than via runtime URL/path inspection; see the individual variant
/// docs for the meaning of each.
///
/// `Version` and `DetailedVersion` are kept separate so the on-disk distinction
/// between the bare string form `"*"` and the table form `{ version = "*" }`
/// survives a serialization round-trip.
///
/// The binary variants do not carry matchspec selectors: a `.conda` archive
/// is already a fully-specified package, so `version`, `build`, etc. would be
/// meaningless.
#[derive(Debug, Clone, Hash, ::serde::Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum PixiSpec {
    /// The spec is represented solely by a version string. The package should
    /// be retrieved from a channel.
    ///
    /// This is similar to the `DetailedVersion` variant but with a simplified
    /// version spec.
    Version(VersionSpec),

    /// The spec is represented by a detailed version spec. The package should
    /// be retrieved from a channel.
    DetailedVersion(Box<DetailedSpec>),

    /// URL pointing at a binary conda archive (`.conda` / `.tar.bz2`).
    UrlBinary(UrlBinarySpec),

    /// Local path pointing at a binary conda archive (`.conda` / `.tar.bz2`).
    PathBinary(PathBinarySpec),

    /// URL pointing at a source archive (e.g. `.zip`, `.tar.gz`), with
    /// optional matchspec selectors used to pick which built output to use.
    UrlSource(Box<UrlSourceSpec>),

    /// Local directory or source path, with optional matchspec selectors.
    PathSource(Box<PathSourceSpec>),

    /// Git repository, with optional matchspec selectors.
    Git(Box<GitSpec>),
}

impl Default for PixiSpec {
    fn default() -> Self {
        PixiSpec::any()
    }
}

impl Display for PixiSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PixiSpec::Version(version) => write!(f, "{version}"),
            PixiSpec::DetailedVersion(detailed) => write!(f, "{detailed}"),
            PixiSpec::UrlBinary(url) => write!(f, "{url}"),
            PixiSpec::PathBinary(path) => write!(f, "{path}"),
            PixiSpec::UrlSource(url) => write!(f, "{url}"),
            PixiSpec::PathSource(path) => write!(f, "{path}"),
            PixiSpec::Git(git) => write!(f, "{git}"),
        }
    }
}

impl From<VersionSpec> for PixiSpec {
    fn from(value: VersionSpec) -> Self {
        Self::Version(value)
    }
}

impl PixiSpec {
    /// Creates a new instance that matches any version.
    pub const fn any() -> Self {
        Self::Version(VersionSpec::Any)
    }

    /// Convert a [`NamelessMatchSpec`] into a [`PixiSpec`].
    pub fn from_nameless_matchspec(
        spec: NamelessMatchSpec,
        channel_config: &ChannelConfig,
    ) -> Self {
        if let Some(url) = spec.url {
            // Detect whether the URL points to a binary archive.
            if url_is_binary(&url) {
                Self::UrlBinary(UrlBinarySpec {
                    url,
                    md5: spec.md5,
                    sha256: spec.sha256,
                })
            } else {
                Self::UrlSource(Box::new(UrlSourceSpec {
                    url,
                    md5: spec.md5,
                    sha256: spec.sha256,
                    subdirectory: Subdirectory::default(),
                    matchspec: MatchspecFields {
                        version: spec.version,
                        build: spec.build,
                        build_number: spec.build_number,
                        extras: spec.extras,
                        flags: spec.flags,
                        subdir: spec.subdir,
                        license: spec.license,
                        condition: spec.condition,
                        track_features: spec.track_features,
                    },
                }))
            }
        } else {
            // A bare nameless spec (no fields other than possibly `version`)
            // round-trips to a `PixiSpec::Version`, so it serializes as the
            // bare string `"*"` (or `">=1"`, ...) instead of a table.
            let is_bare = spec.build.is_none()
                && spec.build_number.is_none()
                && spec.file_name.is_none()
                && spec.extras.is_none()
                && spec.flags.is_none()
                && spec.channel.is_none()
                && spec.subdir.is_none()
                && spec.md5.is_none()
                && spec.sha256.is_none()
                && spec.license.is_none()
                && spec.license_family.is_none()
                && spec.condition.is_none()
                && spec.track_features.is_none();
            if is_bare {
                return Self::Version(spec.version.unwrap_or(VersionSpec::Any));
            }
            Self::DetailedVersion(Box::new(DetailedSpec {
                version: spec.version,
                build: spec.build,
                build_number: spec.build_number,
                file_name: spec.file_name,
                extras: spec.extras,
                flags: spec.flags,
                channel: spec.channel.map(|c| {
                    NamedChannelOrUrl::from_str(&channel_config.canonical_name(c.base_url.url()))
                        .unwrap()
                }),
                subdir: spec.subdir,
                md5: spec.md5,
                sha256: spec.sha256,
                license: spec.license,
                license_family: spec.license_family,
                condition: spec.condition,
                track_features: spec.track_features,
            }))
        }
    }

    /// Returns true if this spec has a version spec. `*` does not count as a
    /// valid version spec.
    pub fn has_version_spec(&self) -> bool {
        match self {
            Self::Version(v) => v != &VersionSpec::Any,
            Self::DetailedVersion(v) => v.version.as_ref().is_some_and(|v| v != &VersionSpec::Any),
            Self::UrlSource(u) => u
                .matchspec
                .version
                .as_ref()
                .is_some_and(|v| v != &VersionSpec::Any),
            Self::PathSource(p) => p
                .matchspec
                .version
                .as_ref()
                .is_some_and(|v| v != &VersionSpec::Any),
            Self::Git(g) => g
                .matchspec
                .version
                .as_ref()
                .is_some_and(|v| v != &VersionSpec::Any),
            _ => false,
        }
    }

    /// Returns a [`VersionSpec`] if this instance carries one.
    pub fn as_version_spec(&self) -> Option<&VersionSpec> {
        match self {
            Self::Version(v) => Some(v),
            Self::DetailedVersion(v) => v.version.as_ref(),
            Self::UrlSource(u) => u.matchspec.version.as_ref(),
            Self::PathSource(p) => p.matchspec.version.as_ref(),
            Self::Git(g) => g.matchspec.version.as_ref(),
            _ => None,
        }
    }

    /// Returns a [`DetailedSpec`] if this instance is a detailed spec.
    pub fn as_detailed(&self) -> Option<&DetailedSpec> {
        match self {
            Self::DetailedVersion(v) => Some(v),
            _ => None,
        }
    }

    /// Returns the requested extras for this spec, if any are declared.
    ///
    /// Extras can be declared on a detailed version spec as well as on the
    /// source spec variants (URL, path, git) through their matchspec
    /// selectors.
    pub fn extras(&self) -> Option<&[String]> {
        let extras = match self {
            Self::DetailedVersion(v) => v.extras.as_ref(),
            Self::UrlSource(u) => u.matchspec.extras.as_ref(),
            Self::PathSource(p) => p.matchspec.extras.as_ref(),
            Self::Git(g) => g.matchspec.extras.as_ref(),
            Self::Version(_) | Self::UrlBinary(_) | Self::PathBinary(_) => None,
        };
        extras.map(Vec::as_slice)
    }

    /// Returns a [`UrlSourceSpec`] if this instance is a URL source spec.
    pub fn as_url_source(&self) -> Option<&UrlSourceSpec> {
        match self {
            Self::UrlSource(u) => Some(u),
            _ => None,
        }
    }

    /// Returns a [`UrlBinarySpec`] if this instance is a URL binary spec.
    pub fn as_url_binary(&self) -> Option<&UrlBinarySpec> {
        match self {
            Self::UrlBinary(u) => Some(u),
            _ => None,
        }
    }

    /// Returns a [`PathSourceSpec`] if this instance is a path source spec.
    pub fn as_path_source(&self) -> Option<&PathSourceSpec> {
        match self {
            Self::PathSource(p) => Some(p),
            _ => None,
        }
    }

    /// Returns a [`PathBinarySpec`] if this instance is a path binary spec.
    pub fn as_path_binary(&self) -> Option<&PathBinarySpec> {
        match self {
            Self::PathBinary(p) => Some(p),
            _ => None,
        }
    }

    /// Returns a [`GitSpec`] if this instance is a git spec.
    pub fn as_git(&self) -> Option<&GitSpec> {
        match self {
            Self::Git(g) => Some(g),
            _ => None,
        }
    }

    /// Returns a [`UrlSpec`] reconstructed from a URL-typed variant.
    pub fn as_url(&self) -> Option<UrlSpec> {
        match self {
            Self::UrlBinary(u) => Some(UrlSpec {
                url: u.url.clone(),
                md5: u.md5,
                sha256: u.sha256,
                subdirectory: Subdirectory::default(),
            }),
            Self::UrlSource(u) => Some(UrlSpec {
                url: u.url.clone(),
                md5: u.md5,
                sha256: u.sha256,
                subdirectory: u.subdirectory.clone(),
            }),
            _ => None,
        }
    }

    /// Returns a [`PathSpec`] reconstructed from a path-typed variant.
    pub fn as_path(&self) -> Option<PathSpec> {
        match self {
            Self::PathBinary(p) => Some(PathSpec {
                path: p.path.clone(),
            }),
            Self::PathSource(p) => Some(PathSpec {
                path: p.path.clone(),
            }),
            _ => None,
        }
    }

    /// Converts this instance into a [`VersionSpec`] if possible.
    pub fn into_version(self) -> Option<VersionSpec> {
        match self {
            Self::Version(v) => Some(v),
            Self::DetailedVersion(v) => v.version,
            Self::UrlSource(u) => u.matchspec.version,
            Self::PathSource(p) => p.matchspec.version,
            Self::Git(g) => g.matchspec.version,
            _ => None,
        }
    }

    /// Converts this instance into a [`DetailedSpec`] if possible.
    pub fn into_detailed(self) -> Option<DetailedSpec> {
        match self {
            Self::DetailedVersion(v) => Some(*v),
            Self::Version(v) => Some(DetailedSpec {
                version: Some(v),
                ..DetailedSpec::default()
            }),
            _ => None,
        }
    }

    /// Converts this instance into a [`UrlSpec`] if possible.
    pub fn into_url(self) -> Option<UrlSpec> {
        match self {
            Self::UrlBinary(u) => Some(UrlSpec {
                url: u.url,
                md5: u.md5,
                sha256: u.sha256,
                subdirectory: Subdirectory::default(),
            }),
            Self::UrlSource(u) => Some(UrlSpec {
                url: u.url,
                md5: u.md5,
                sha256: u.sha256,
                subdirectory: u.subdirectory,
            }),
            _ => None,
        }
    }

    /// Converts this instance into a [`GitSpec`] if possible.
    pub fn into_git(self) -> Option<GitSpec> {
        match self {
            Self::Git(g) => Some(*g),
            _ => None,
        }
    }

    /// Converts this instance into a [`PathSpec`] if possible.
    pub fn into_path(self) -> Option<PathSpec> {
        match self {
            Self::PathBinary(p) => Some(PathSpec { path: p.path }),
            Self::PathSource(p) => Some(PathSpec { path: p.path }),
            _ => None,
        }
    }

    /// Convert this instance into a binary spec.
    ///
    /// A binary spec always refers to a binary package.
    pub fn try_into_nameless_match_spec(
        self,
        channel_config: &ChannelConfig,
    ) -> Result<Option<NamelessMatchSpec>, SpecConversionError> {
        let spec = match self {
            PixiSpec::Version(version) => Some(NamelessMatchSpec {
                version: Some(version),
                ..NamelessMatchSpec::default()
            }),
            PixiSpec::DetailedVersion(spec) => {
                Some(spec.try_into_nameless_match_spec(channel_config)?)
            }
            PixiSpec::UrlBinary(url) => Some(url.into()),
            PixiSpec::PathBinary(path) => {
                Some(path.try_into_nameless_match_spec(&channel_config.root_dir)?)
            }
            PixiSpec::UrlSource(_) | PixiSpec::PathSource(_) | PixiSpec::Git(_) => None,
        };

        Ok(spec)
    }

    /// Converts this instance into a source or binary spec.
    pub fn into_source_or_binary(self) -> Either<SourceSpec, BinarySpec> {
        match self {
            PixiSpec::Version(version) => Either::Right(BinarySpec::Version(version)),
            PixiSpec::DetailedVersion(detailed) => {
                Either::Right(BinarySpec::DetailedVersion(detailed))
            }
            PixiSpec::UrlBinary(url) => Either::Right(BinarySpec::Url(url)),
            PixiSpec::PathBinary(path) => Either::Right(BinarySpec::Path(path)),
            PixiSpec::UrlSource(url) => Either::Left(SourceSpec::from(*url)),
            PixiSpec::PathSource(path) => Either::Left(SourceSpec::from(*path)),
            PixiSpec::Git(git) => Either::Left(SourceSpec::from(*git)),
        }
    }

    /// Converts this instance into a source spec if this instance represents a
    /// source package.
    #[allow(clippy::result_large_err)]
    pub fn try_into_source_spec(self) -> Result<SourceSpec, Self> {
        match self {
            PixiSpec::UrlSource(url) => Ok(SourceSpec::from(*url)),
            PixiSpec::PathSource(path) => Ok(SourceSpec::from(*path)),
            PixiSpec::Git(git) => Ok(SourceSpec::from(*git)),
            _ => Err(self),
        }
    }

    /// Returns true if this spec represents a binary package.
    pub fn is_binary(&self) -> bool {
        matches!(
            self,
            Self::Version(_) | Self::DetailedVersion(_) | Self::UrlBinary(_) | Self::PathBinary(_)
        )
    }

    /// Returns true if this spec represents a source package.
    pub fn is_source(&self) -> bool {
        !self.is_binary()
    }

    /// Returns true if this spec represents a mutable source.
    /// A spec is mutable if it points to a local path-based source
    /// (non-binary).
    pub fn is_mutable(&self) -> bool {
        matches!(self, Self::PathSource(_))
    }

    /// Converts this instance into a [`toml_edit::Value`].
    pub fn to_toml_value(&self) -> toml_edit::Value {
        ::serde::Serialize::serialize(self, toml_edit::ser::ValueSerializer::new())
            .expect("conversion to toml cannot fail")
    }

    /// Returns a [`NamelessMatchSpec`] that represents this spec disregarding
    /// any source specification.
    pub fn try_into_nameless_match_spec_ref(
        self,
        channel_config: &ChannelConfig,
    ) -> Result<NamelessMatchSpec, SpecConversionError> {
        match self.into_source_or_binary() {
            Either::Left(source) => Ok(source.to_nameless_match_spec()),
            Either::Right(binary) => binary.try_into_nameless_match_spec(channel_config),
        }
    }

    /// Convert this spec into a fully-named [`MatchSpec`] for the given
    /// package `name`.
    pub fn to_match_spec(
        self,
        name: &PackageName,
        channel_config: &ChannelConfig,
    ) -> Result<MatchSpec, SpecConversionError> {
        let nameless = match self.into_source_or_binary() {
            Either::Left(source) => source.to_nameless_match_spec(),
            Either::Right(binary) => binary.try_into_nameless_match_spec(channel_config)?,
        };
        Ok(MatchSpec::from_nameless(nameless, name.clone().into()))
    }
}

/// A source location, without any match-spec selectors.
///
/// This is enough to locate and check out the source. It deliberately does
/// not carry [`MatchspecFields`]: where only the location matters (checking
/// out source, referring to a manifest), use this type. To also constrain
/// which built output is selected, pair it with [`MatchspecFields`] in a
/// [`SourceSpec`].
#[derive(Debug, Clone, Hash, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum SourceLocationSpec {
    /// The spec is represented as an archive that can be downloaded from the
    /// specified URL.
    Url(UrlSpec),

    /// The spec is represented as a git repository.
    Git(GitLocationSpec),

    /// The spec is represented as a local directory or local file archive.
    Path(PathSpec),
}

impl Display for SourceLocationSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceLocationSpec::Url(url) => write!(f, "{url}"),
            SourceLocationSpec::Git(git) => write!(f, "{git}"),
            SourceLocationSpec::Path(path) => write!(f, "{path}"),
        }
    }
}

impl SourceLocationSpec {
    /// Returns true if this location is a git repository.
    pub fn is_git(&self) -> bool {
        matches!(self, SourceLocationSpec::Git(_))
    }

    /// Returns true if this location is a local path.
    pub fn is_path(&self) -> bool {
        matches!(self, SourceLocationSpec::Path(_))
    }

    /// Returns true if this location is a URL archive.
    pub fn is_url(&self) -> bool {
        matches!(self, SourceLocationSpec::Url(_))
    }

    /// Attaches match-spec selectors to this location.
    pub fn with_matchspec(self, matchspec: MatchspecFields) -> SourceSpec {
        SourceSpec {
            location: self,
            matchspec,
        }
    }

    /// Converts this instance into a [`toml_edit::Value`].
    pub fn to_toml_value(&self) -> toml_edit::Value {
        ::serde::Serialize::serialize(self, toml_edit::ser::ValueSerializer::new())
            .expect("conversion to toml cannot fail")
    }

    /// Resolves the source location using the provided source anchor.
    pub fn resolve(self, source_anchor: &SourceAnchor) -> Self {
        source_anchor.resolve_location(self)
    }
}

impl From<UrlSpec> for SourceLocationSpec {
    fn from(value: UrlSpec) -> Self {
        SourceLocationSpec::Url(value)
    }
}

impl From<PathSpec> for SourceLocationSpec {
    fn from(value: PathSpec) -> Self {
        SourceLocationSpec::Path(value)
    }
}

impl From<GitLocationSpec> for SourceLocationSpec {
    fn from(value: GitLocationSpec) -> Self {
        SourceLocationSpec::Git(value)
    }
}

/// A specification for a source package: a [`SourceLocationSpec`] together
/// with the [`MatchspecFields`] selectors that pick which built output of
/// that source to use.
#[derive(Debug, Clone, Hash, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceSpec {
    /// Where the source is located.
    #[serde(flatten)]
    pub location: SourceLocationSpec,

    /// Match-spec selectors applied to the built output.
    #[serde(flatten)]
    pub matchspec: MatchspecFields,
}

impl Display for SourceSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.location)
    }
}

impl SourceSpec {
    /// Returns true if this spec represents a git repository.
    pub fn is_git(&self) -> bool {
        self.location.is_git()
    }

    /// Returns true if this spec represents a local path.
    pub fn is_path(&self) -> bool {
        self.location.is_path()
    }

    /// Returns true if this spec represents a URL archive.
    pub fn is_url(&self) -> bool {
        self.location.is_url()
    }

    /// Convert this source location into a [`NamelessMatchSpec`] containing
    /// only the matchspec selectors (the location itself is not encoded).
    pub fn to_nameless_match_spec(&self) -> NamelessMatchSpec {
        self.matchspec.to_nameless_match_spec()
    }

    /// Converts this instance into a [`toml_edit::Value`].
    pub fn to_toml_value(&self) -> toml_edit::Value {
        ::serde::Serialize::serialize(self, toml_edit::ser::ValueSerializer::new())
            .expect("conversion to toml cannot fail")
    }

    /// Resolves the source location using the provided source anchor.
    pub fn resolve(self, source_anchor: &SourceAnchor) -> Self {
        source_anchor.resolve(self)
    }
}

impl From<UrlSourceSpec> for SourceSpec {
    fn from(value: UrlSourceSpec) -> Self {
        let UrlSourceSpec {
            url,
            md5,
            sha256,
            subdirectory,
            matchspec,
        } = value;
        SourceSpec {
            location: SourceLocationSpec::Url(UrlSpec {
                url,
                md5,
                sha256,
                subdirectory,
            }),
            matchspec,
        }
    }
}

impl From<PathSourceSpec> for SourceSpec {
    fn from(value: PathSourceSpec) -> Self {
        SourceSpec {
            location: SourceLocationSpec::Path(PathSpec { path: value.path }),
            matchspec: value.matchspec,
        }
    }
}

impl From<GitSpec> for SourceSpec {
    fn from(value: GitSpec) -> Self {
        let matchspec = value.matchspec;
        SourceSpec {
            location: SourceLocationSpec::Git(GitLocationSpec {
                git: value.git,
                rev: value.rev,
                subdirectory: value.subdirectory,
            }),
            matchspec,
        }
    }
}

impl From<SourceLocationSpec> for SourceSpec {
    fn from(location: SourceLocationSpec) -> Self {
        SourceSpec {
            location,
            matchspec: MatchspecFields::default(),
        }
    }
}

impl From<SourceSpec> for PixiSpec {
    fn from(value: SourceSpec) -> Self {
        let SourceSpec {
            location,
            matchspec,
        } = value;
        match location {
            SourceLocationSpec::Url(UrlSpec {
                url,
                md5,
                sha256,
                subdirectory,
            }) => Self::UrlSource(Box::new(UrlSourceSpec {
                url,
                md5,
                sha256,
                subdirectory,
                matchspec,
            })),
            SourceLocationSpec::Git(git) => Self::Git(Box::new(git.with_matchspec(matchspec))),
            SourceLocationSpec::Path(PathSpec { path }) => {
                Self::PathSource(Box::new(PathSourceSpec { path, matchspec }))
            }
        }
    }
}

impl From<DetailedSpec> for PixiSpec {
    fn from(value: DetailedSpec) -> Self {
        Self::DetailedVersion(Box::new(value))
    }
}

impl From<UrlSpec> for PixiSpec {
    fn from(value: UrlSpec) -> Self {
        // A bare `UrlSpec` doesn't pre-commit to source-or-binary, so we
        // inspect the URL to decide which typed variant to produce.
        if url_is_binary(&value.url) {
            Self::UrlBinary(UrlBinarySpec {
                url: value.url,
                md5: value.md5,
                sha256: value.sha256,
            })
        } else {
            Self::UrlSource(Box::new(UrlSourceSpec {
                url: value.url,
                md5: value.md5,
                sha256: value.sha256,
                subdirectory: value.subdirectory,
                matchspec: MatchspecFields::default(),
            }))
        }
    }
}

impl From<UrlSourceSpec> for PixiSpec {
    fn from(value: UrlSourceSpec) -> Self {
        Self::UrlSource(Box::new(value))
    }
}

impl From<UrlBinarySpec> for PixiSpec {
    fn from(value: UrlBinarySpec) -> Self {
        Self::UrlBinary(value)
    }
}

impl From<GitSpec> for PixiSpec {
    fn from(value: GitSpec) -> Self {
        Self::Git(Box::new(value))
    }
}

impl From<PathSpec> for PixiSpec {
    fn from(value: PathSpec) -> Self {
        // Inspect the path extension to decide source-or-binary.
        if CondaArchiveType::try_from(std::path::Path::new(value.path.as_str())).is_some() {
            Self::PathBinary(PathBinarySpec { path: value.path })
        } else {
            Self::PathSource(Box::new(PathSourceSpec {
                path: value.path,
                matchspec: MatchspecFields::default(),
            }))
        }
    }
}

impl From<PathSourceSpec> for PixiSpec {
    fn from(value: PathSourceSpec) -> Self {
        Self::PathSource(Box::new(value))
    }
}

impl From<PathBinarySpec> for PixiSpec {
    fn from(value: PathBinarySpec) -> Self {
        Self::PathBinary(value)
    }
}

impl From<PixiSpec> for toml_edit::Value {
    fn from(value: PixiSpec) -> Self {
        ::serde::Serialize::serialize(&value, toml_edit::ser::ValueSerializer::new())
            .expect("conversion to toml cannot fail")
    }
}

/// A specification for a binary package.
///
/// This type only represents binary packages. Use [`PixiSpec`] to represent
/// both binary and source packages.
#[derive(Debug, Clone, Hash, PartialEq, Eq, ::serde::Serialize)]
#[serde(untagged)]
pub enum BinarySpec {
    /// The spec is represented solely by a version string. The package should
    /// be retrieved from a channel.
    Version(VersionSpec),

    /// The spec is represented by a detailed version spec. The package should
    /// be retrieved from a channel.
    DetailedVersion(Box<DetailedSpec>),

    /// The spec is represented as an archive that can be downloaded from the
    /// specified URL.
    Url(UrlBinarySpec),

    /// The spec is represented as a local path. The package should be retrieved
    /// from the local filesystem.
    Path(PathBinarySpec),
}

impl BinarySpec {
    /// Constructs a new instance that matches anything.
    pub const fn any() -> Self {
        Self::Version(VersionSpec::Any)
    }

    /// Convert this instance into a binary spec.
    ///
    /// A binary spec always refers to a binary package.
    pub fn try_into_nameless_match_spec(
        self,
        channel_config: &ChannelConfig,
    ) -> Result<NamelessMatchSpec, SpecConversionError> {
        match self {
            BinarySpec::Version(version) => Ok(NamelessMatchSpec {
                version: Some(version),
                ..NamelessMatchSpec::default()
            }),
            BinarySpec::DetailedVersion(spec) => spec.try_into_nameless_match_spec(channel_config),
            BinarySpec::Url(url) => Ok(url.into()),
            BinarySpec::Path(path) => path.try_into_nameless_match_spec(&channel_config.root_dir),
        }
    }

    /// Convert this binary spec into a fully-named [`MatchSpec`] for the
    /// given package `name`.
    pub fn to_match_spec(
        self,
        name: &PackageName,
        channel_config: &ChannelConfig,
    ) -> Result<MatchSpec, SpecConversionError> {
        let nameless = self.try_into_nameless_match_spec(channel_config)?;
        Ok(MatchSpec::from_nameless(nameless, name.clone().into()))
    }
}

impl From<BinarySpec> for PixiSpec {
    fn from(value: BinarySpec) -> Self {
        match value {
            BinarySpec::Version(version) => Self::Version(version),
            BinarySpec::DetailedVersion(detailed) => Self::DetailedVersion(detailed),
            BinarySpec::Url(url) => Self::UrlBinary(url),
            BinarySpec::Path(path) => Self::PathBinary(path),
        }
    }
}

impl From<VersionSpec> for BinarySpec {
    fn from(value: VersionSpec) -> Self {
        Self::Version(value)
    }
}

#[cfg(feature = "rattler_lock")]
impl From<rattler_lock::source::SourceLocation> for SourceLocationSpec {
    fn from(value: rattler_lock::source::SourceLocation) -> Self {
        match value {
            rattler_lock::source::SourceLocation::Url(url) => SourceLocationSpec::Url(url.into()),
            rattler_lock::source::SourceLocation::Git(git) => SourceLocationSpec::Git(git.into()),
            rattler_lock::source::SourceLocation::Path(path) => {
                SourceLocationSpec::Path(path.into())
            }
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl From<rattler_lock::source::SourceLocation> for SourceSpec {
    fn from(value: rattler_lock::source::SourceLocation) -> Self {
        SourceLocationSpec::from(value).into()
    }
}

#[cfg(feature = "rattler_lock")]
impl From<SourceLocationSpec> for rattler_lock::source::SourceLocation {
    fn from(value: SourceLocationSpec) -> Self {
        match value {
            SourceLocationSpec::Url(url) => Self::Url(url.into()),
            SourceLocationSpec::Git(git) => Self::Git(git.into()),
            SourceLocationSpec::Path(path) => Self::Path(path.into()),
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl From<SourceSpec> for rattler_lock::source::SourceLocation {
    fn from(value: SourceSpec) -> Self {
        value.location.into()
    }
}

#[cfg(feature = "rattler_lock")]
impl From<rattler_lock::source::UrlSourceLocation> for UrlSpec {
    fn from(value: rattler_lock::source::UrlSourceLocation) -> Self {
        let rattler_lock::source::UrlSourceLocation {
            url,
            md5,
            sha256,
            subdirectory,
        } = value;

        Self {
            url,
            md5,
            sha256,
            subdirectory: subdirectory
                .and_then(|s| Subdirectory::try_from(s).ok())
                .unwrap_or_default(),
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl From<UrlSpec> for rattler_lock::source::UrlSourceLocation {
    fn from(value: UrlSpec) -> Self {
        Self {
            url: value.url,
            md5: value.md5,
            sha256: value.sha256,
            subdirectory: value.subdirectory.to_option_string(),
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl From<rattler_lock::source::GitSourceLocation> for GitLocationSpec {
    fn from(value: rattler_lock::source::GitSourceLocation) -> Self {
        Self {
            git: value.git,
            rev: match value.rev {
                Some(rattler_lock::source::GitReference::Branch(branch)) => {
                    Some(GitReference::Branch(branch))
                }
                Some(rattler_lock::source::GitReference::Tag(tag)) => Some(GitReference::Tag(tag)),
                Some(rattler_lock::source::GitReference::Rev(rev)) => Some(GitReference::Rev(rev)),
                None => None,
            },
            subdirectory: value
                .subdirectory
                .and_then(|s| Subdirectory::try_from(s).ok())
                .unwrap_or_default(),
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl From<GitLocationSpec> for rattler_lock::source::GitSourceLocation {
    fn from(value: GitLocationSpec) -> Self {
        Self {
            git: value.git,
            rev: match value.rev {
                Some(GitReference::Branch(branch)) => {
                    Some(rattler_lock::source::GitReference::Branch(branch))
                }
                Some(GitReference::Tag(tag)) => Some(rattler_lock::source::GitReference::Tag(tag)),
                Some(GitReference::Rev(rev)) => Some(rattler_lock::source::GitReference::Rev(rev)),
                Some(GitReference::DefaultBranch) | None => None,
            },
            subdirectory: value.subdirectory.to_option_string(),
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl From<rattler_lock::source::PathSourceLocation> for PathSpec {
    fn from(value: rattler_lock::source::PathSourceLocation) -> Self {
        Self { path: value.path }
    }
}

#[cfg(feature = "rattler_lock")]
impl From<PathSpec> for rattler_lock::source::PathSourceLocation {
    fn from(value: PathSpec) -> Self {
        Self { path: value.path }
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use rattler_conda_types::{
        ChannelConfig, MatchSpec, MatchSpecCondition, NamelessMatchSpec, PackageName,
        ParseMatchSpecOptions, ParseStrictness::Lenient, RepodataRevision, StringMatcher,
        VersionSpec,
    };
    use serde::Serialize;
    use serde_json::{Value, json};
    use url::Url;

    use crate::{BinarySpec, MatchspecFields, PixiSpec};

    #[test]
    fn test_is_binary() {
        let binary_packages = [
            json! { "1.2.3" },
            json!({ "version": "1.2.3" }),
            json! { "*" },
            json!({ "version": "1.2.3", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "url": "https://conda.anaconda.org/conda-forge/linux-64/21cmfast-3.3.1-py38h0db86a8_1.conda" }),
        ];

        for binary_package in binary_packages {
            let spec: PixiSpec = serde_json::from_value(binary_package.clone()).unwrap();
            assert!(
                spec.is_binary(),
                "{binary_package} should be a binary package"
            );
            assert!(
                !spec.is_source(),
                "{binary_package} should not be a source package"
            );
        }

        let source_packages = [
            json!({ "path": "foobar" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "branch": "main" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "tag": "v1" }),
            json!({ "url": "https://github.com/conda-forge/21cmfast-feedstock.zip" }),
        ];

        for source_package in source_packages {
            let spec: PixiSpec = serde_json::from_value(source_package.clone()).unwrap();
            assert!(spec.is_source(), "{spec:?} should be a source package");
            assert!(!spec.is_binary(), "{spec:?} should not be a binary package");
        }
    }

    #[test]
    fn test_into_nameless_match_spec() {
        let examples = [
            // Should be identified as binary packages.
            json!({ "version": "1.2.3" }),
            json!({ "version": "1.2.3", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "subdir": "linux-64" }),
            json!({ "channel": "conda-forge", "subdir": "linux-64" }),
            json!({ "channel": "conda-forge", "subdir": "linux-64" }),
            json!({ "url": "https://conda.anaconda.org/conda-forge/linux-64/21cmfast-3.3.1-py38h0db86a8_1.conda" }),
            json!({ "url": "https://conda.anaconda.org/conda-forge/linux-64/21cmfast-3.3.1-py38h0db86a8_1.conda", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "path": "21cmfast-3.3.1-py38h0db86a8_1.conda" }),
            json!({ "path": "packages/foo/.././21cmfast-3.3.1-py38h0db86a8_1.conda" }),
            json!({ "url": "file:///21cmfast-3.3.1-py38h0db86a8_1.conda" }),
            json!({ "path": "~/foo/../21cmfast-3.3.1-py38h0db86a8_1.conda" }),
            // Should not be binary packages.
            json!({ "path": "foobar" }),
            json!({ "path": "~/.cache" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "branch": "main" }),
            json!({ "url": "http://github.com/conda-forge/21cmfast-feedstock/releases/21cmfast-3.3.1-py38h0db86a8_1.zip" }),
        ];

        #[derive(Serialize)]
        struct Snapshot {
            input: Value,
            result: Value,
        }

        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let mut snapshot = Vec::new();
        for input in examples {
            let spec: PixiSpec = serde_json::from_value(input.clone()).unwrap();
            let result = match spec.try_into_nameless_match_spec(&channel_config) {
                Ok(spec) => serde_json::to_value(spec).unwrap(),
                Err(err) => {
                    json!({ "error": err.to_string() })
                }
            };
            snapshot.push(Snapshot { input, result });
        }

        let path = Url::from_directory_path(channel_config.root_dir).unwrap();
        let home_dir = Url::from_directory_path(dirs::home_dir().unwrap()).unwrap();
        insta::with_settings!({filters => vec![
            (path.as_str(), "file://<ROOT>/"),
            (home_dir.as_str(), "file://<HOME>/"),
        ]}, {
            insta::assert_yaml_snapshot!(snapshot);
        });
    }

    #[test]
    fn test_pixi_spec_to_match_spec_binary() {
        // A version-only PixiSpec for `numpy` should produce
        // `MatchSpec("numpy >=1.0")` with the supplied name attached.
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let spec: PixiSpec = serde_json::from_value(json!({ "version": ">=1.0" })).unwrap();
        let name = PackageName::new_unchecked("numpy");
        let match_spec = spec.to_match_spec(&name, &channel_config).unwrap();
        assert_eq!(
            match_spec.name.as_exact().map(PackageName::as_normalized),
            Some("numpy")
        );
        assert_eq!(match_spec.to_string(), "numpy >=1.0");
    }

    #[test]
    fn test_v3_nameless_match_spec_fields_roundtrip() {
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let condition = MatchSpecCondition::MatchSpec(Box::new(
            MatchSpec::from_str(
                "python >=3.12",
                ParseMatchSpecOptions::lenient()
                    .with_repodata_revision(RepodataRevision::V3)
                    .with_experimental_conditionals(true),
            )
            .unwrap(),
        ));
        let spec = NamelessMatchSpec {
            version: Some(VersionSpec::from_str(">=1.0", Lenient).unwrap()),
            extras: Some(vec!["cuda".to_string(), "mkl".to_string()]),
            flags: Some(vec![
                StringMatcher::from_str("cuda").unwrap(),
                StringMatcher::from_str("blas:*").unwrap(),
            ]),
            license_family: Some("BSD".to_string()),
            condition: Some(condition.clone()),
            track_features: Some(vec!["legacy".to_string()]),
            ..NamelessMatchSpec::default()
        };

        let pixi_spec = PixiSpec::from_nameless_matchspec(spec.clone(), &channel_config);
        assert!(matches!(pixi_spec, PixiSpec::DetailedVersion(_)));

        let roundtrip = pixi_spec
            .try_into_nameless_match_spec(&channel_config)
            .unwrap()
            .unwrap();
        assert_eq!(roundtrip.version, spec.version);
        assert_eq!(roundtrip.extras, spec.extras);
        assert_eq!(roundtrip.flags, spec.flags);
        assert_eq!(roundtrip.license_family, spec.license_family);
        assert_eq!(roundtrip.condition, spec.condition);
        assert_eq!(roundtrip.track_features, spec.track_features);
    }

    #[test]
    fn test_pixi_spec_to_match_spec_source_path() {
        // A path-based source spec carries no version material, so the
        // resulting MatchSpec only names the package.
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let spec: PixiSpec = serde_json::from_value(json!({ "path": "foobar" })).unwrap();
        let name = PackageName::new_unchecked("my-package");
        let match_spec = spec.to_match_spec(&name, &channel_config).unwrap();
        assert_eq!(
            match_spec.name.as_exact().map(PackageName::as_normalized),
            Some("my-package")
        );
    }

    #[test]
    fn test_binary_spec_to_match_spec() {
        // The BinarySpec convenience method should produce a MatchSpec
        // equivalent to `try_into_nameless_match_spec` + `from_nameless`.
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let spec = BinarySpec::Version(VersionSpec::from_str(">=2.0", Lenient).unwrap());
        let name = PackageName::new_unchecked("openssl");
        let match_spec = spec.to_match_spec(&name, &channel_config).unwrap();
        assert_eq!(match_spec.to_string(), "openssl >=2.0");
    }

    /// A path source spec carrying matchspec selectors should turn into a
    /// `MatchSpec` carrying those same selectors (the location itself
    /// is dropped — `to_match_spec` returns a *binary-shaped* matchspec
    /// that the solver uses to pick which built output of the source
    /// satisfies the constraint).
    #[test]
    fn test_path_source_with_matchspec_to_match_spec() {
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let spec: PixiSpec = serde_json::from_value(json!({
            "path": "../my-pkg",
            "version": ">=1.0",
            "build": "py310_*",
            "subdir": "linux-64",
        }))
        .unwrap();

        let name = PackageName::new_unchecked("my-pkg");
        let match_spec = spec.to_match_spec(&name, &channel_config).unwrap();

        assert_eq!(
            match_spec.name.as_exact().map(PackageName::as_normalized),
            Some("my-pkg")
        );
        assert_eq!(match_spec.version.unwrap().to_string(), ">=1.0");
        assert_eq!(match_spec.subdir.as_deref(), Some("linux-64"));
        assert!(match_spec.build.is_some());
    }

    /// `try_into_nameless_match_spec` on a source variant returns `None`
    /// (sources don't have a fully-resolved nameless matchspec); use
    /// `to_match_spec` or read `matchspec()` instead.
    #[test]
    fn test_try_into_nameless_match_spec_source_returns_none() {
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());

        let spec: PixiSpec = serde_json::from_value(json!({
            "git": "https://example.com/foo.git",
            "version": ">=1.0",
        }))
        .unwrap();
        assert!(matches!(spec, PixiSpec::Git(_)));
        let result = spec.try_into_nameless_match_spec(&channel_config).unwrap();
        assert!(result.is_none());
    }

    /// `MatchspecFields::from_nameless_match_spec` extracts only the
    /// matchspec subset; binary-only fields (`url`, `md5`, `sha256`,
    /// `file_name`, `channel`, `namespace`) are dropped.
    #[test]
    fn test_matchspec_fields_extraction_drops_binary_fields() {
        use rattler_conda_types::NamelessMatchSpec;
        let nameless = NamelessMatchSpec {
            version: Some(VersionSpec::from_str(">=1.0", Lenient).unwrap()),
            build_number: Some(">=3".parse().unwrap()),
            file_name: Some("foo.conda".to_string()),
            md5: Some(Default::default()),
            ..NamelessMatchSpec::default()
        };
        let fields = MatchspecFields::from_nameless_match_spec(&nameless);
        assert_eq!(fields.version.as_ref().unwrap().to_string(), ">=1.0");
        assert!(fields.build_number.is_some());
        // Re-encoding to NamelessMatchSpec must NOT carry the binary-only
        // fields back.
        let round_tripped = fields.to_nameless_match_spec();
        assert!(round_tripped.file_name.is_none());
        assert!(round_tripped.md5.is_none());
        assert!(round_tripped.url.is_none());
    }

    /// A bare version string round-trips to a `PixiSpec::Version` and
    /// serializes back to a string, while the table form `{ version = "X" }`
    /// stays a table - the two on-disk shapes are preserved by the type.
    #[test]
    fn test_bare_version_vs_table_form_preserved() {
        let from_string: PixiSpec = serde_json::from_value(json!("1.2.3")).unwrap();
        assert!(matches!(from_string, PixiSpec::Version(_)));
        let json = serde_json::to_value(&from_string).unwrap();
        assert_eq!(json, json!("==1.2.3"));
        assert_eq!(from_string.to_string(), "==1.2.3");

        let from_table: PixiSpec = serde_json::from_value(json!({ "version": "1.2.3" })).unwrap();
        assert!(matches!(from_table, PixiSpec::DetailedVersion(_)));
        let json = serde_json::to_value(&from_table).unwrap();
        assert!(json.is_object(), "expected a JSON object, got {json}");
    }

    /// `from_nameless_matchspec` on a bare nameless spec (no fields other
    /// than possibly `version`) produces a `PixiSpec::Version`, which
    /// serializes as the bare string form.
    #[test]
    fn test_bare_nameless_becomes_version() {
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let nameless = MatchSpec::from_str("any-spec", ParseMatchSpecOptions::lenient())
            .unwrap()
            .into_nameless()
            .1;
        let spec = PixiSpec::from_nameless_matchspec(nameless, &channel_config);
        assert!(matches!(spec, PixiSpec::Version(_)));
        let json = serde_json::to_value(&spec).unwrap();
        assert_eq!(json, json!("*"));
        assert_eq!(spec.to_string(), "*");
    }

    /// A `Detailed` spec with *any* extra field present must serialize
    /// as a table — the bare-string collapse only applies when only
    /// `version` is set.
    #[test]
    fn test_detailed_with_extra_field_serializes_as_table() {
        let spec: PixiSpec = serde_json::from_value(json!({
            "version": "1.2.3",
            "build": "py37_*",
        }))
        .unwrap();
        let value = serde_json::to_value(&spec).unwrap();
        assert!(value.is_object(), "expected a JSON object, got {value}");
    }
}
