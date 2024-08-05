#![deny(missing_docs)]

//! This crate defines the `Spec` type which represents a package specification
//! for pixi.
//!
//! The `Spec` type represents the user input for a package. It can represent
//! both source and binary packages. The `Spec` type can optionally be converted
//! to a `NamelessMatchSpec` which is used to match binary packages.

mod serde;

use std::{
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use rattler_conda_types::{
    package::ArchiveIdentifier, BuildNumberSpec, ChannelConfig, NamedChannelOrUrl,
    NamelessMatchSpec, StringMatcher, VersionSpec,
};
use rattler_digest::{Md5Hash, Sha256Hash};
use serde_with::{serde_as, skip_serializing_none};
use thiserror::Error;
use typed_path::{Utf8NativePathBuf, Utf8TypedPathBuf};
use url::Url;

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
}

/// A package specification for pixi.
///
/// This type can represent both source and binary packages. Use the
/// [`Self::to_nameless_match_spec`] method to convert this type into a type
/// that only represents binary packages.
#[derive(Debug, Clone, Hash, ::serde::Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Spec {
    /// The spec is represented soley by a version string. The package should be
    /// retrieved from a channel.
    ///
    /// This is similar to the `DetailedVersion` variant but with a simplified
    /// version spec.
    Version(VersionSpec),

    /// The spec is represented by a detailed version spec. The package should
    /// be retrieved from a channel.
    DetailedVersion(DetailedVersionSpec),

    /// The spec is represented as an archive that can be downloaded from the
    /// specified URL. The package should be retrieved from the URL and can
    /// either represent a source or binary package depending on the archive
    /// type.
    Url(UrlSpec),

    /// The spec is represented as a git repository. The package represents a
    /// source distribution of some kind.
    Git(GitSpec),

    /// The spec is represented as a local path. The package should be retrieved
    /// from the local filesystem. The package can be either a source or binary
    /// package.
    Path(PathSpec),
}

impl Default for Spec {
    fn default() -> Self {
        Spec::Version(VersionSpec::Any)
    }
}

impl From<VersionSpec> for Spec {
    fn from(value: VersionSpec) -> Self {
        Self::Version(value)
    }
}

impl From<NamelessMatchSpec> for Spec {
    fn from(value: NamelessMatchSpec) -> Self {
        if let Some(url) = value.url {
            Self::Url(UrlSpec {
                url,
                md5: value.md5,
                sha256: value.sha256,
            })
        } else if value.build.is_none()
            && value.build_number.is_none()
            && value.file_name.is_none()
            && value.channel.is_none()
            && value.subdir.is_none()
            && value.md5.is_none()
            && value.sha256.is_none()
        {
            Self::Version(value.version.unwrap_or(VersionSpec::Any))
        } else {
            Self::DetailedVersion(DetailedVersionSpec {
                version: value.version.unwrap_or(VersionSpec::Any),
                build: value.build,
                build_number: value.build_number,
                file_name: value.file_name,
                channel: value
                    .channel
                    .map(|c| NamedChannelOrUrl::from_str(c.name()).unwrap()),
                subdir: value.subdir,
                md5: value.md5,
                sha256: value.sha256,
            })
        }
    }
}

impl Spec {
    /// Returns true if this spec has a version spec. `*` does not count as a
    /// valid version spec.
    pub fn has_version_spec(&self) -> bool {
        match self {
            Self::Version(v) => v != &VersionSpec::Any,
            Self::DetailedVersion(v) => v.version != VersionSpec::Any,
            _ => false,
        }
    }

    /// Returns a [`VersionSpec`] if this instance is a version spec.
    pub fn as_version(&self) -> Option<&VersionSpec> {
        match self {
            Self::Version(v) => Some(v),
            Self::DetailedVersion(v) => Some(&v.version),
            _ => None,
        }
    }

    /// Returns a [`DetailedVersionSpec`] if this instance is a detailed version
    /// spec.
    pub fn as_detailed_version(&self) -> Option<&DetailedVersionSpec> {
        match self {
            Self::DetailedVersion(v) => Some(v),
            _ => None,
        }
    }

    /// Returns a [`UrlSpec`] if this instance is a detailed version spec.
    pub fn as_url(&self) -> Option<&UrlSpec> {
        match self {
            Self::Url(v) => Some(v),
            _ => None,
        }
    }

    /// Returns a [`GitSpec`] if this instance is a git spec.
    pub fn as_git(&self) -> Option<&GitSpec> {
        match self {
            Self::Git(v) => Some(v),
            _ => None,
        }
    }

    /// Returns a [`PathSpec`] if this instance is a path spec.
    pub fn as_path(&self) -> Option<&PathSpec> {
        match self {
            Self::Path(v) => Some(v),
            _ => None,
        }
    }

    /// Converts this instance into a [`VersionSpec`] if possible.
    pub fn into_version(self) -> Option<VersionSpec> {
        match self {
            Self::Version(v) => Some(v),
            _ => None,
        }
    }

    /// Converts this instance into a [`DetailedVersionSpec`] if possible.
    pub fn into_detailed_version(self) -> Option<DetailedVersionSpec> {
        match self {
            Self::DetailedVersion(v) => Some(v),
            Self::Version(v) => Some(DetailedVersionSpec {
                version: v,
                ..DetailedVersionSpec::default()
            }),
            _ => None,
        }
    }

    /// Converts this instance into a [`UrlSpec`] if possible.
    pub fn into_url(self) -> Option<UrlSpec> {
        match self {
            Self::Url(v) => Some(v),
            _ => None,
        }
    }

    /// Converts this instance into a [`GitSpec`] if possible.
    pub fn into_git(self) -> Option<GitSpec> {
        match self {
            Self::Git(v) => Some(v),
            _ => None,
        }
    }

    /// Converts this instance into a [`PathSpec`] if possible.
    pub fn into_path(self) -> Option<PathSpec> {
        match self {
            Self::Path(v) => Some(v),
            _ => None,
        }
    }

    /// Convert this instance into a binary spec.
    ///
    /// A binary spec always refers to a binary package.
    pub fn into_nameless_match_spec(
        self,
        channel_config: &ChannelConfig,
    ) -> Result<Option<NamelessMatchSpec>, SpecConversionError> {
        let spec = match self {
            Spec::Version(version) => Some(NamelessMatchSpec {
                version: Some(version),
                ..NamelessMatchSpec::default()
            }),
            Spec::DetailedVersion(spec) => Some(spec.into_nameless_match_spec(channel_config)),
            Spec::Url(url) => url.try_into_nameless_match_spec().ok(),
            Spec::Git(_) => None,
            Spec::Path(path) => path.try_into_nameless_match_spec(&channel_config.root_dir)?,
        };

        Ok(spec)
    }

    /// Converts this instance into a source spec if this instance represents a
    /// source package.
    #[allow(clippy::result_large_err)]
    pub fn into_source_spec(self) -> Result<SourceSpec, Self> {
        match self {
            Spec::Url(url) => url
                .try_into_source_url()
                .map(SourceSpec::from)
                .map_err(Spec::from),
            Spec::Git(git) => Ok(SourceSpec::Git(git)),
            Spec::Path(path) => path
                .try_into_source_path()
                .map(SourceSpec::from)
                .map_err(Spec::from),
            _ => Err(self),
        }
    }

    /// Returns true if this spec represents a binary package.
    pub fn is_binary(&self) -> bool {
        match self {
            Self::Version(_) => true,
            Self::DetailedVersion(_) => true,
            Self::Url(url) => url.is_binary(),
            Self::Git(_) => false,
            Self::Path(path) => path.is_binary(),
        }
    }

    /// Returns true if this spec represents a source package.
    pub fn is_source(&self) -> bool {
        !self.is_binary()
    }

    #[cfg(feature = "toml_edit")]
    /// Converts this instance into a [`toml_edit::Value`].
    pub fn to_toml_value(&self) -> toml_edit::Value {
        ::serde::Serialize::serialize(self, toml_edit::ser::ValueSerializer::new())
            .expect("conversion to toml cannot fail")
    }
}

/// A specification for a source package.
///
/// This type only represents source packages. Use [`Spec`] to represent both
/// binary and source packages.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum SourceSpec {
    /// The spec is represented as an archive that can be downloaded from the
    /// specified URL.
    Url(UrlSourceSpec),

    /// The spec is represented as a git repository.
    Git(GitSpec),

    /// The spec is represented as a local directory or local file archive.
    Path(PathSourceSpec),
}

impl From<SourceSpec> for Spec {
    fn from(value: SourceSpec) -> Self {
        match value {
            SourceSpec::Url(url) => Self::Url(url.into()),
            SourceSpec::Git(git) => Self::Git(git),
            SourceSpec::Path(path) => Self::Path(path.into()),
        }
    }
}

/// A specification for a package in a conda channel.
///
/// This type maps closely to [`rattler_conda_types::NamelessMatchSpec`] but
/// does not represent a `url` field. To represent a `url` spec, use [`UrlSpec`]
/// instead.
#[serde_as]
#[skip_serializing_none]
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DetailedVersionSpec {
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    pub version: VersionSpec,

    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub build: Option<StringMatcher>,

    /// The build number of the package
    pub build_number: Option<BuildNumberSpec>,

    /// Match the specific filename of the package
    pub file_name: Option<String>,

    /// The channel of the package
    pub channel: Option<NamedChannelOrUrl>,

    /// The subdir of the channel
    pub subdir: Option<String>,

    /// The md5 hash of the package
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the package
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,
}

impl DetailedVersionSpec {
    /// Converts this instance into a [`NamelessMatchSpec`].
    pub fn into_nameless_match_spec(self, channel_config: &ChannelConfig) -> NamelessMatchSpec {
        NamelessMatchSpec {
            version: Some(self.version),
            build: self.build,
            build_number: self.build_number,
            file_name: self.file_name,
            channel: self
                .channel
                .map(|c| c.into_channel(channel_config))
                .map(Arc::new),
            subdir: self.subdir,
            namespace: None,
            md5: self.md5,
            sha256: self.sha256,
            url: None,
        }
    }
}

impl Default for DetailedVersionSpec {
    fn default() -> Self {
        Self {
            version: VersionSpec::Any,
            build: None,
            build_number: None,
            file_name: None,
            channel: None,
            subdir: None,
            md5: None,
            sha256: None,
        }
    }
}

impl From<DetailedVersionSpec> for Spec {
    fn from(value: DetailedVersionSpec) -> Self {
        Self::DetailedVersion(value)
    }
}

/// A specification of a package from a URL. This is used to represent both
/// source and binary packages.
#[serde_as]
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
pub struct UrlSpec {
    /// The URL of the package
    pub url: Url,

    /// The md5 hash of the package
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the package
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,
}

impl UrlSpec {
    /// Converts this instance into a [`NamelessMatchSpec`] if the URL points to
    /// a binary package.
    #[allow(clippy::result_large_err)]
    pub fn try_into_nameless_match_spec(self) -> Result<NamelessMatchSpec, Self> {
        if self.is_binary() {
            Ok(NamelessMatchSpec {
                url: Some(self.url),
                md5: self.md5,
                sha256: self.sha256,
                ..NamelessMatchSpec::default()
            })
        } else {
            Err(self)
        }
    }

    /// Converts this instance into a [`SourceUrlSpec`] if the URL points to a
    /// source package. Otherwise, returns this instance unmodified.
    #[allow(clippy::result_large_err)]
    pub fn try_into_source_url(self) -> Result<UrlSourceSpec, Self> {
        if self.is_binary() {
            Err(self)
        } else {
            Ok(UrlSourceSpec {
                url: self.url,
                md5: self.md5,
                sha256: self.sha256,
            })
        }
    }

    /// Returns true if the URL points to a binary package.
    pub fn is_binary(&self) -> bool {
        ArchiveIdentifier::try_from_url(&self.url).is_some()
    }
}

impl From<UrlSpec> for Spec {
    fn from(value: UrlSpec) -> Self {
        Self::Url(value)
    }
}

/// A specification of a source archive from a URL.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct UrlSourceSpec {
    /// The URL of the package
    pub url: Url,

    /// The md5 hash of the archive
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the archive
    pub sha256: Option<Sha256Hash>,
}

impl From<UrlSourceSpec> for UrlSpec {
    fn from(value: UrlSourceSpec) -> Self {
        Self {
            url: value.url,
            md5: value.md5,
            sha256: value.sha256,
        }
    }
}

impl From<UrlSourceSpec> for SourceSpec {
    fn from(value: UrlSourceSpec) -> Self {
        SourceSpec::Url(value)
    }
}

/// A specification of a package from a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GitSpec {
    /// The git url of the package
    pub git: Url,

    /// The git revision of the package
    #[serde(skip_serializing_if = "Option::is_none", flatten)]
    pub rev: Option<GitReference>,
}

impl From<GitSpec> for Spec {
    fn from(value: GitSpec) -> Self {
        Self::Git(value)
    }
}

/// A reference to a specific commit in a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitReference {
    /// The HEAD commit of a branch.
    Branch(String),

    /// A specific tag.
    Tag(String),

    /// A specific commit.
    Rev(String),
}

/// A specification of a package from a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PathSpec {
    /// The path to the package
    pub path: Utf8TypedPathBuf,
}

impl PathSpec {
    /// Converts this instance into a [`NamelessMatchSpec`] if the path points
    /// to binary archive.
    pub fn try_into_nameless_match_spec(
        self,
        root_dir: &Path,
    ) -> Result<Option<NamelessMatchSpec>, SpecConversionError> {
        if !self.is_binary() {
            // Not a binary package
            return Ok(None);
        }

        // Convert the path to an absolute path based on the channel config
        let path = if self.path.is_absolute() {
            self.path
        } else {
            let Some(root_dir_str) = root_dir.to_str() else {
                return Err(SpecConversionError::NotUtf8RootDir(root_dir.to_path_buf()));
            };
            let native_root_dir = Utf8NativePathBuf::from(root_dir_str);
            if !native_root_dir.is_absolute() {
                return Err(SpecConversionError::NonAbsoluteRootDir(
                    root_dir.to_path_buf(),
                ));
            }

            native_root_dir.to_typed_path().join(self.path).normalize()
        };

        // Convert the absolute url to a file:// url
        let local_file_url =
            file_url::file_path_to_url(path.to_path()).expect("failed to convert path to file url");

        Ok(Some(NamelessMatchSpec {
            url: Some(local_file_url),
            ..NamelessMatchSpec::default()
        }))
    }

    /// Converts this instance into a [`PathSourceSpec`] if the path points to a
    /// source package. Otherwise, returns this instance unmodified.
    #[allow(clippy::result_large_err)]
    pub fn try_into_source_path(self) -> Result<PathSourceSpec, Self> {
        if self.is_binary() {
            Err(self)
        } else {
            Ok(PathSourceSpec { path: self.path })
        }
    }

    /// Returns true if this path points to a binary archive.
    pub fn is_binary(&self) -> bool {
        self.path
            .file_name()
            .and_then(ArchiveIdentifier::try_from_path)
            .is_none()
    }
}

impl From<PathSpec> for Spec {
    fn from(value: PathSpec) -> Self {
        Self::Path(value)
    }
}

/// Path to a source package. Different from [`PathSpec`] in that this type only
/// refers to source packages.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PathSourceSpec {
    /// The path to the package. Either a directory or an archive.
    pub path: Utf8TypedPathBuf,
}

impl From<PathSourceSpec> for PathSpec {
    fn from(value: PathSourceSpec) -> Self {
        Self { path: value.path }
    }
}

impl From<PathSourceSpec> for SourceSpec {
    fn from(value: PathSourceSpec) -> Self {
        Self::Path(value)
    }
}

#[cfg(feature = "toml_edit")]
impl From<Spec> for toml_edit::Value {
    fn from(value: Spec) -> Self {
        ::serde::Serialize::serialize(&value, toml_edit::ser::ValueSerializer::new())
            .expect("conversion to toml cannot fail")
    }
}

#[cfg(test)]
mod test {
    use rattler_conda_types::ChannelConfig;
    use serde::Serialize;
    use serde_json::{json, Value};
    use url::Url;

    use crate::Spec;

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
            let spec: Spec = serde_json::from_value(binary_package).unwrap();
            assert!(spec.is_binary());
            assert!(!spec.is_source());
        }

        let source_packages = [
            json!({ "path": "foobar" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "branch": "main" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "tag": "v1" }),
            json!({ "url": "https://github.com/conda-forge/21cmfast-feedstock.zip" }),
        ];

        for source_package in source_packages {
            let spec: Spec = serde_json::from_value(source_package).unwrap();
            assert!(spec.is_source());
            assert!(!spec.is_binary());
        }
    }

    #[test]
    fn test_into_nameless_match_spec() {
        let examples = [
            // Should be identified as binary packages.
            json!({ "version": "1.2.3" }),
            json!({ "version": "1.2.3", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "url": "https://conda.anaconda.org/conda-forge/linux-64/21cmfast-3.3.1-py38h0db86a8_1.conda" }),
            json!({ "url": "https://conda.anaconda.org/conda-forge/linux-64/21cmfast-3.3.1-py38h0db86a8_1.conda", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "path": "21cmfast-3.3.1-py38h0db86a8_1.conda" }),
            json!({ "url": "file:///21cmfast-3.3.1-py38h0db86a8_1.conda" }),
            // Should not be binary packages.
            json!({ "path": "foobar" }),
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
            let spec: Spec = serde_json::from_value(input.clone()).unwrap();
            let result = match spec.into_nameless_match_spec(&channel_config) {
                Ok(spec) => serde_json::to_value(spec).unwrap(),
                Err(err) => {
                    json!({ "error": err.to_string() })
                }
            };
            snapshot.push(Snapshot { input, result });
        }

        let path = Url::from_directory_path(channel_config.root_dir).unwrap();
        insta::with_settings!({filters => vec![
            (path.as_str(), "file://<ROOT>/"),
        ]}, {
            insta::assert_yaml_snapshot!(snapshot);
        });
    }
}
