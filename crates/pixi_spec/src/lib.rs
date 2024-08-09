#![deny(missing_docs)]

//! This crate defines the [`PixiSpec`] type which represents a package
//! specification for pixi.
//!
//! The `PixiSpec` type represents the user input for a package. It can
//! represent both source and binary packages. The `PixiSpec` type can
//! optionally be converted to a `NamelessMatchSpec` which is used to match
//! binary packages.

mod detailed;
mod git;
mod path;
mod serde;
mod url;

use std::{path::PathBuf, str::FromStr};

pub use detailed::DetailedSpec;
pub use git::{GitReference, GitSpec};
pub use path::{PathSourceSpec, PathSpec};
use rattler_conda_types::{ChannelConfig, NamedChannelOrUrl, NamelessMatchSpec, VersionSpec};
use thiserror::Error;
pub use url::{UrlSourceSpec, UrlSpec};

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
}

/// A package specification for pixi.
///
/// This type can represent both source and binary packages. Use the
/// [`Self::try_into_nameless_match_spec`] method to convert this type into a
/// type that only represents binary packages.
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
    DetailedVersion(DetailedSpec),

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

impl Default for PixiSpec {
    fn default() -> Self {
        PixiSpec::Version(VersionSpec::Any)
    }
}

impl From<VersionSpec> for PixiSpec {
    fn from(value: VersionSpec) -> Self {
        Self::Version(value)
    }
}

impl PixiSpec {
    /// Convert a [`NamelessMatchSpec`] into a [`PixiSpec`].
    pub fn from_nameless_matchspec(
        spec: NamelessMatchSpec,
        channel_config: &ChannelConfig,
    ) -> Self {
        if let Some(url) = spec.url {
            Self::Url(UrlSpec {
                url,
                md5: spec.md5,
                sha256: spec.sha256,
            })
        } else if spec.build.is_none()
            && spec.build_number.is_none()
            && spec.file_name.is_none()
            && spec.channel.is_none()
            && spec.subdir.is_none()
            && spec.md5.is_none()
            && spec.sha256.is_none()
        {
            Self::Version(spec.version.unwrap_or(VersionSpec::Any))
        } else {
            Self::DetailedVersion(DetailedSpec {
                version: spec.version,
                build: spec.build,
                build_number: spec.build_number,
                file_name: spec.file_name,
                channel: spec.channel.map(|c| {
                    NamedChannelOrUrl::from_str(&channel_config.canonical_name(c.base_url()))
                        .unwrap()
                }),
                subdir: spec.subdir,
                md5: spec.md5,
                sha256: spec.sha256,
            })
        }
    }

    /// Returns true if this spec has a version spec. `*` does not count as a
    /// valid version spec.
    pub fn has_version_spec(&self) -> bool {
        match self {
            Self::Version(v) => v != &VersionSpec::Any,
            Self::DetailedVersion(v) => v.version.as_ref().is_some_and(|v| v != &VersionSpec::Any),
            _ => false,
        }
    }

    /// Returns a [`VersionSpec`] if this instance is a version spec.
    pub fn as_version_spec(&self) -> Option<&VersionSpec> {
        match self {
            Self::Version(v) => Some(v),
            Self::DetailedVersion(v) => v.version.as_ref(),
            _ => None,
        }
    }

    /// Returns a [`DetailedSpec`] if this instance is a detailed version
    /// spec.
    pub fn as_detailed(&self) -> Option<&DetailedSpec> {
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
            Self::DetailedVersion(DetailedSpec {
                version: Some(v), ..
            }) => Some(v),
            _ => None,
        }
    }

    /// Converts this instance into a [`DetailedSpec`] if possible.
    pub fn into_detailed(self) -> Option<DetailedSpec> {
        match self {
            Self::DetailedVersion(v) => Some(v),
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
    pub fn try_into_nameless_match_spec(
        self,
        channel_config: &ChannelConfig,
    ) -> Result<Option<NamelessMatchSpec>, SpecConversionError> {
        let spec = match self {
            PixiSpec::Version(version) => Some(NamelessMatchSpec {
                version: Some(version),
                ..NamelessMatchSpec::default()
            }),
            PixiSpec::DetailedVersion(spec) => Some(spec.into_nameless_match_spec(channel_config)),
            PixiSpec::Url(url) => url.try_into_nameless_match_spec().ok(),
            PixiSpec::Git(_) => None,
            PixiSpec::Path(path) => path.try_into_nameless_match_spec(&channel_config.root_dir)?,
        };

        Ok(spec)
    }

    /// Converts this instance into a source spec if this instance represents a
    /// source package.
    #[allow(clippy::result_large_err)]
    pub fn try_into_source_spec(self) -> Result<SourceSpec, Self> {
        match self {
            PixiSpec::Url(url) => url
                .try_into_source_url()
                .map(SourceSpec::from)
                .map_err(PixiSpec::from),
            PixiSpec::Git(git) => Ok(SourceSpec::Git(git)),
            PixiSpec::Path(path) => path
                .try_into_source_path()
                .map(SourceSpec::from)
                .map_err(PixiSpec::from),
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
/// This type only represents source packages. Use [`PixiSpec`] to represent
/// both binary and source packages.
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

impl From<SourceSpec> for PixiSpec {
    fn from(value: SourceSpec) -> Self {
        match value {
            SourceSpec::Url(url) => Self::Url(url.into()),
            SourceSpec::Git(git) => Self::Git(git),
            SourceSpec::Path(path) => Self::Path(path.into()),
        }
    }
}

impl From<DetailedSpec> for PixiSpec {
    fn from(value: DetailedSpec) -> Self {
        Self::DetailedVersion(value)
    }
}

impl From<UrlSpec> for PixiSpec {
    fn from(value: UrlSpec) -> Self {
        Self::Url(value)
    }
}

impl From<UrlSourceSpec> for SourceSpec {
    fn from(value: UrlSourceSpec) -> Self {
        SourceSpec::Url(value)
    }
}

impl From<GitSpec> for PixiSpec {
    fn from(value: GitSpec) -> Self {
        Self::Git(value)
    }
}

impl From<PathSpec> for PixiSpec {
    fn from(value: PathSpec) -> Self {
        Self::Path(value)
    }
}

impl From<PathSourceSpec> for SourceSpec {
    fn from(value: PathSourceSpec) -> Self {
        Self::Path(value)
    }
}

#[cfg(feature = "toml_edit")]
impl From<PixiSpec> for toml_edit::Value {
    fn from(value: PixiSpec) -> Self {
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

    use crate::PixiSpec;

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
}
