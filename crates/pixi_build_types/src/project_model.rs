//! This module is a collection of types that represent a pixi package in a protocol
//! format that can be sent over the wire.
//! We need to vendor a lot of the types, and simplify them in some cases, so that
//! we have a stable protocol that can be used to communicate in the build tasks.
//!
//! The rationale is that we want to have a stable protocol to provide forwards and backwards compatibility.
//! The idea for **backwards compatibility** is that we try not to break this in pixi as much as possible.
//! So as long as older pixi TOMLs keep loading, we can send them to the backend.
//!
//! In regards to forwards compatibility, we want to be able to keep converting to all versions of the `VersionedProjectModel`
//! as much as possible.
//!
//! This is why we append a `V{version}` to the type names, to indicate the version
//! of the protocol.
//!
//! Only the whole ProjectModel is versioned explicitly in an enum.
//! When making a change to one of the types, be sure to add another enum declaration if it is breaking.
use ordermap::OrderMap;
use rattler_conda_types::{BuildNumberSpec, StringMatcher, Version, VersionSpec};
use rattler_digest::{Md5, Md5Hash, Sha256, Sha256Hash, serde::SerializableHash};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::{DeserializeFromStr, DisplayFromStr, SerializeDisplay};
use std::convert::Infallible;
use std::fmt::Display;
use std::hash::Hash;
use std::path::PathBuf;
use std::str::FromStr;
use url::Url;

/// Enum containing all versions of the project model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "version", content = "data")]
#[serde(rename_all = "camelCase")]
pub enum VersionedProjectModel {
    /// Version 1 of the project model.
    #[serde(rename = "1")]
    V1(ProjectModelV1),
    // When adding don't forget to update the highest_version function
}

impl VersionedProjectModel {
    /// Highest version of the project model.
    pub fn highest_version() -> u32 {
        // increase this when adding a new version
        1
    }

    /// Move into the v1 type, returns None if the version is not v1.
    pub fn into_v1(self) -> Option<ProjectModelV1> {
        match self {
            VersionedProjectModel::V1(v) => Some(v),
            // Add this once we have more versions
            //_ => None,
        }
    }

    /// Returns a reference to the v1 type, returns None if the version is not v1.
    pub fn as_v1(&self) -> Option<&ProjectModelV1> {
        match self {
            VersionedProjectModel::V1(v) => Some(v),
            // Add this once we have more versions
            //_ => None,
        }
    }
}

/// The source package name of a package. Not normalized per se.
pub type SourcePackageName = String;

#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelV1 {
    /// The name of the project
    pub name: String,

    /// The version of the project
    pub version: Option<Version>,

    /// An optional project description
    pub description: Option<String>,

    /// Optional authors
    pub authors: Option<Vec<String>>,

    /// The license as a valid SPDX string (e.g. MIT AND Apache-2.0)
    pub license: Option<String>,

    /// The license file (relative to the project root)
    pub license_file: Option<PathBuf>,

    /// Path to the README file of the project (relative to the project root)
    pub readme: Option<PathBuf>,

    /// URL of the project homepage
    pub homepage: Option<Url>,

    /// URL of the project source repository
    pub repository: Option<Url>,

    /// URL of the project documentation
    pub documentation: Option<Url>,

    /// The target of the project, this may contain
    /// platform specific configurations.
    pub targets: Option<TargetsV1>,
}

impl From<ProjectModelV1> for VersionedProjectModel {
    fn from(value: ProjectModelV1) -> Self {
        VersionedProjectModel::V1(value)
    }
}

/// Represents a target selector. Currently, we only support explicit platform
/// selection.
#[derive(Debug, Clone, DeserializeFromStr, SerializeDisplay, Hash, Eq, PartialEq)]
pub enum TargetSelectorV1 {
    // Platform specific configuration
    Unix,
    Linux,
    Win,
    MacOs,
    Platform(String),
    // TODO: Add minijinja coolness here.
}

impl Display for TargetSelectorV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetSelectorV1::Unix => write!(f, "unix"),
            TargetSelectorV1::Linux => write!(f, "linux"),
            TargetSelectorV1::Win => write!(f, "win"),
            TargetSelectorV1::MacOs => write!(f, "macos"),
            TargetSelectorV1::Platform(p) => write!(f, "{}", p),
        }
    }
}
impl FromStr for TargetSelectorV1 {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "unix" => Ok(TargetSelectorV1::Unix),
            "linux" => Ok(TargetSelectorV1::Linux),
            "win" => Ok(TargetSelectorV1::Win),
            "macos" => Ok(TargetSelectorV1::MacOs),
            _ => Ok(TargetSelectorV1::Platform(s.to_string())),
        }
    }
}

/// A collect of targets including a default target.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Hash, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetsV1 {
    pub default_target: Option<TargetV1>,

    /// We use an [`OrderMap`] to preserve the order in which the items where
    /// defined in the manifest.
    pub targets: Option<OrderMap<TargetSelectorV1, TargetV1>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Hash, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetV1 {
    /// Host dependencies of the project
    pub host_dependencies: Option<OrderMap<SourcePackageName, PackageSpecV1>>,

    /// Build dependencies of the project
    pub build_dependencies: Option<OrderMap<SourcePackageName, PackageSpecV1>>,

    /// Run dependencies of the project
    pub run_dependencies: Option<OrderMap<SourcePackageName, PackageSpecV1>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum PackageSpecV1 {
    /// This is a binary dependency
    Binary(Box<BinaryPackageSpecV1>),
    /// This is a dependency on a source package
    Source(SourcePackageSpecV1),
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub enum SourcePackageSpecV1 {
    /// The spec is represented as an archive that can be downloaded from the
    /// specified URL. The package should be retrieved from the URL and can
    /// either represent a source or binary package depending on the archive
    /// type.
    Url(UrlSpecV1),

    /// The spec is represented as a git repository. The package represents a
    /// source distribution of some kind.
    Git(GitSpecV1),

    /// The spec is represented as a local path. The package should be retrieved
    /// from the local filesystem. The package can be either a source or binary
    /// package.
    Path(PathSpecV1),
}

#[serde_as]
#[derive(Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UrlSpecV1 {
    /// The URL of the package
    pub url: Url,

    /// The md5 hash of the package
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the package
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,
}

impl std::fmt::Debug for UrlSpecV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug_struct = f.debug_struct("UrlSpecV1");

        debug_struct.field("url", &self.url);
        if let Some(md5) = &self.md5 {
            debug_struct.field("md5", &format!("{:x}", md5));
        }
        if let Some(sha256) = &self.sha256 {
            debug_struct.field("sha256", &format!("{:x}", sha256));
        }
        debug_struct.finish()
    }
}

/// A specification of a package from a git repository.
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitSpecV1 {
    /// The git url of the package which can contain git+ prefixes.
    pub git: Url,

    /// The git revision of the package
    pub rev: Option<GitReferenceV1>,

    /// The git subdirectory of the package
    pub subdirectory: Option<String>,
}

/// A specification of a package from a path
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PathSpecV1 {
    /// The path to the package
    pub path: String,
}

/// A reference to a specific commit in a git repository.
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum GitReferenceV1 {
    /// The HEAD commit of a branch.
    Branch(String),

    /// A specific tag.
    Tag(String),

    /// A specific commit.
    Rev(String),

    /// A default branch.
    DefaultBranch,
}

/// Similar to a [`rattler_conda_types::NamelessMatchSpec`]
#[serde_as]
#[derive(Clone, Serialize, Deserialize, Default, Hash, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BinaryPackageSpecV1 {
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub version: Option<VersionSpec>,
    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub build: Option<StringMatcher>,
    /// The build number of the package
    pub build_number: Option<BuildNumberSpec>,
    /// Match the specific filename of the package
    pub file_name: Option<String>,
    /// The channel of the package
    pub channel: Option<Url>,
    /// The subdir of the channel
    pub subdir: Option<String>,
    /// The md5 hash of the package
    #[serde_as(as = "Option<SerializableHash<Md5>>")]
    pub md5: Option<Md5Hash>,
    /// The sha256 hash of the package
    #[serde_as(as = "Option<SerializableHash<Sha256>>")]
    pub sha256: Option<Sha256Hash>,
}

impl From<VersionSpec> for BinaryPackageSpecV1 {
    fn from(value: VersionSpec) -> Self {
        Self {
            version: Some(value),
            ..Default::default()
        }
    }
}

impl From<&VersionSpec> for BinaryPackageSpecV1 {
    fn from(value: &VersionSpec) -> Self {
        Self {
            version: Some(value.clone()),
            ..Default::default()
        }
    }
}

impl std::fmt::Debug for BinaryPackageSpecV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug_struct = f.debug_struct("NamelessMatchSpecV1");

        if let Some(version) = &self.version {
            debug_struct.field("version", version);
        }
        if let Some(build) = &self.build {
            debug_struct.field("build", build);
        }
        if let Some(build_number) = &self.build_number {
            debug_struct.field("build_number", build_number);
        }
        if let Some(file_name) = &self.file_name {
            debug_struct.field("file_name", file_name);
        }
        if let Some(channel) = &self.channel {
            debug_struct.field("channel", channel);
        }
        if let Some(subdir) = &self.subdir {
            debug_struct.field("subdir", subdir);
        }
        if let Some(md5) = &self.md5 {
            debug_struct.field("md5", &format!("{:x}", md5));
        }
        if let Some(sha256) = &self.sha256 {
            debug_struct.field("sha256", &format!("{:x}", sha256));
        }

        debug_struct.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_sample_target_v1() -> TargetV1 {
        TargetV1 {
            host_dependencies: Some(OrderMap::from([(
                "host_dep1".to_string(),
                PackageSpecV1::Binary(Box::default()),
            )])),
            build_dependencies: Some(OrderMap::from([(
                "build_dep1".to_string(),
                PackageSpecV1::Binary(Box::default()),
            )])),
            run_dependencies: Some(OrderMap::from([(
                "run_dep1".to_string(),
                PackageSpecV1::Binary(Box::default()),
            )])),
        }
    }

    #[test]
    fn serialize_targets_v1_with_default_target() {
        let targets = TargetsV1 {
            default_target: Some(create_sample_target_v1()),
            targets: None,
        };

        let serialized = serde_json::to_string(&targets).unwrap();
        assert!(serialized.contains("defaultTarget"));
        assert!(serialized.contains("hostDependencies"));
    }

    #[test]
    fn serialize_targets_v1_with_multiple_targets() {
        let platform_strs = [
            "unix",
            "win",
            "macos",
            "linux-64",
            "linux-arm64",
            "linux-ppc64le",
            "osx-64",
            "osx-arm64",
            "win-64",
            "win-arm64",
        ];

        let targets = TargetsV1 {
            default_target: None,
            targets: Some(
                platform_strs
                    .iter()
                    .map(|s| {
                        let selector = match *s {
                            "unix" => TargetSelectorV1::Unix,
                            "win" => TargetSelectorV1::Win,
                            "macos" => TargetSelectorV1::MacOs,
                            other => TargetSelectorV1::Platform(other.to_string()),
                        };
                        (selector, create_sample_target_v1())
                    })
                    .collect(),
            ),
        };

        let serialized = serde_json::to_string(&targets).unwrap();

        for platform in platform_strs {
            assert!(serialized.contains(platform), "Missing: {}", platform);
        }
    }

    #[test]
    fn deserialize_targets_v1_with_empty_fields() {
        let json = r#"{
            "defaultTarget": null,
            "targets": null
        }"#;

        let deserialized: TargetsV1 = serde_json::from_str(json).unwrap();
        assert!(deserialized.default_target.is_none());
        assert!(deserialized.targets.is_none());
    }

    #[test]
    fn deserialize_targets_v1_with_valid_data() {
        let json = r#"{
            "defaultTarget": {
                "hostDependencies": {
                    "host_dep1": {
                        "binary": {}
                    }
                },
                "buildDependencies": null,
                "runDependencies": null
            },
            "targets": {
                "unix": {
                    "hostDependencies": null,
                    "buildDependencies": null,
                    "runDependencies": null
                }
            }
        }"#;

        let deserialized: TargetsV1 = serde_json::from_str(json).unwrap();
        assert!(deserialized.default_target.is_some());
        assert!(deserialized.targets.is_some());
        assert!(
            deserialized
                .targets
                .unwrap()
                .contains_key(&TargetSelectorV1::Unix)
        );
    }
}
