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
#[derive(Debug, Clone, Hash, ::serde::Serialize)]
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

impl Spec {
    /// Returns a [`VersionSpec`] if this instance is a version spec.
    pub fn as_version(&self) -> Option<&VersionSpec> {
        match self {
            Self::Version(v) => Some(v),
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
            Spec::Url(url) => url.into_nameless_match_spec(),
            Spec::Git(_) => None,
            Spec::Path(path) => path.into_nameless_match_spec(&channel_config.root_dir)?,
        };

        Ok(spec)
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
    pub fn into_nameless_match_spec(self) -> Option<NamelessMatchSpec> {
        if ArchiveIdentifier::try_from_url(&self.url).is_some() {
            Some(NamelessMatchSpec {
                url: Some(self.url),
                md5: self.md5,
                sha256: self.sha256,
                ..NamelessMatchSpec::default()
            })
        } else {
            None
        }
    }
}

impl From<UrlSpec> for Spec {
    fn from(value: UrlSpec) -> Self {
        Self::Url(value)
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
    pub rev: Option<GitRev>,
}

impl From<GitSpec> for Spec {
    fn from(value: GitSpec) -> Self {
        Self::Git(value)
    }
}

/// A reference to a specific commit in a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitRev {
    /// The HEAD commit of a branch.
    Branch(String),

    /// A specific tag.
    Tag(String),

    /// A specific commit.
    Commit(String),
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
    pub fn into_nameless_match_spec(
        self,
        root_dir: &Path,
    ) -> Result<Option<NamelessMatchSpec>, SpecConversionError> {
        if self
            .path
            .file_name()
            .and_then(ArchiveIdentifier::try_from_path)
            .is_none()
        {
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
}

impl From<PathSpec> for Spec {
    fn from(value: PathSpec) -> Self {
        Self::Path(value)
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
