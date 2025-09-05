//! This module is a collection of types that represent a pixi package in a
//! protocol format that can be sent over the wire.
//! We need to vendor a lot of the types, and simplify them in some cases, so
//! that we have a stable protocol that can be used to communicate in the build
//! tasks.
//!
//! The rationale is that we want to have a stable protocol to provide forwards
//! and backwards compatibility. The idea for **backwards compatibility** is
//! that we try not to break this in pixi as much as possible. So as long as
//! older pixi TOMLs keep loading, we can send them to the backend.
//!
//! In regards to forwards compatibility, we want to be able to keep converting
//! to all versions of the `VersionedProjectModel` as much as possible.
//!
//! This is why we append a `V{version}` to the type names, to indicate the
//! version of the protocol.
//!
//! Only the whole ProjectModel is versioned explicitly in an enum.
//! When making a change to one of the types, be sure to add another enum
//! declaration if it is breaking.
use std::{convert::Infallible, fmt::Display, hash::Hash, path::PathBuf, str::FromStr};

use ordermap::OrderMap;
use pixi_stable_hash::{IsDefault, StableHashBuilder};
use rattler_conda_types::{BuildNumberSpec, StringMatcher, Version, VersionSpec};
use rattler_digest::{Md5, Md5Hash, Sha256, Sha256Hash, serde::SerializableHash};
use serde::{Deserialize, Serialize};
use serde_with::{DeserializeFromStr, DisplayFromStr, SerializeDisplay, serde_as};
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

    /// Returns a reference to the v1 type, returns None if the version is not
    /// v1.
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

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelV1 {
    /// The name of the project
    pub name: Option<String>,

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

impl IsDefault for ProjectModelV1 {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self)
    }
}

impl From<ProjectModelV1> for VersionedProjectModel {
    fn from(value: ProjectModelV1) -> Self {
        VersionedProjectModel::V1(value)
    }
}

/// Represents a target selector. Currently, we only support explicit platform
/// selection.
#[derive(Debug, Clone, DeserializeFromStr, SerializeDisplay, Eq, PartialEq)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetsV1 {
    pub default_target: Option<TargetV1>,

    /// We use an [`OrderMap`] to preserve the order in which the items where
    /// defined in the manifest.
    pub targets: Option<OrderMap<TargetSelectorV1, TargetV1>>,
}

impl TargetsV1 {
    /// Check if this targets struct is effectively empty (contains no
    /// meaningful data that should affect the hash).
    pub fn is_empty(&self) -> bool {
        let has_meaningless_default_target =
            self.default_target.as_ref().is_none_or(|t| t.is_empty());
        let has_only_empty_targets = self.targets.as_ref().is_none_or(|t| t.is_empty());

        has_meaningless_default_target && has_only_empty_targets
    }
}

impl IsDefault for TargetsV1 {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        if !self.is_empty() { Some(self) } else { None }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetV1 {
    /// Host dependencies of the project
    pub host_dependencies: Option<OrderMap<SourcePackageName, PackageSpecV1>>,

    /// Build dependencies of the project
    pub build_dependencies: Option<OrderMap<SourcePackageName, PackageSpecV1>>,

    /// Run dependencies of the project
    pub run_dependencies: Option<OrderMap<SourcePackageName, PackageSpecV1>>,
}

impl TargetV1 {
    /// Check if this target is effectively empty (contains no meaningful data
    /// that should affect the hash).
    pub fn is_empty(&self) -> bool {
        let has_no_build_deps = self
            .build_dependencies
            .as_ref()
            .is_none_or(|d| d.is_empty());
        let has_no_host_deps = self.host_dependencies.as_ref().is_none_or(|d| d.is_empty());
        let has_no_run_deps = self.run_dependencies.as_ref().is_none_or(|d| d.is_empty());

        has_no_build_deps && has_no_host_deps && has_no_run_deps
    }
}

impl IsDefault for TargetV1 {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        if !self.is_empty() { Some(self) } else { None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum PackageSpecV1 {
    /// This is a binary dependency
    Binary(Box<BinaryPackageSpecV1>),
    /// This is a dependency on a source package
    Source(SourcePackageSpecV1),
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NamedSpecV1<T> {
    pub name: SourcePackageName,

    #[serde(flatten)]
    pub spec: T,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
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
#[derive(Clone, Serialize, Deserialize, Eq, PartialEq)]
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
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
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
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PathSpecV1 {
    /// The path to the package
    pub path: String,
}

/// A reference to a specific commit in a git repository.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
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
#[derive(Clone, Serialize, Deserialize, Default, Eq, PartialEq)]
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
    /// The URL of the package, if it is available
    pub url: Option<Url>,
    /// The license of the package
    pub license: Option<String>,
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

// Custom Hash implementations that skip default values for stability
impl Hash for ProjectModelV1 {
    /// Custom hash implementation using StableHashBuilder to ensure different
    /// field configurations produce different hashes while maintaining
    /// forward/backward compatibility.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let ProjectModelV1 {
            name,
            version,
            description,
            authors,
            license,
            license_file,
            readme,
            homepage,
            repository,
            documentation,
            targets,
        } = self;

        StableHashBuilder::<H>::new()
            .field("authors", authors)
            .field("description", description)
            .field("documentation", documentation)
            .field("homepage", homepage)
            .field("license", license)
            .field("license_file", license_file)
            .field("name", name)
            .field("readme", readme)
            .field("repository", repository)
            .field("targets", targets)
            .field("version", version)
            .finish(state);
    }
}

impl Hash for TargetSelectorV1 {
    /// Custom hash implementation that uses discriminant values to keep the
    /// hash as stable as possible when adding new enum variants.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            TargetSelectorV1::Unix => 0u8.hash(state),
            TargetSelectorV1::Linux => 1u8.hash(state),
            TargetSelectorV1::Win => 2u8.hash(state),
            TargetSelectorV1::MacOs => 3u8.hash(state),
            TargetSelectorV1::Platform(p) => {
                4u8.hash(state);
                p.hash(state);
            }
        }
    }
}

impl Hash for TargetsV1 {
    /// Custom hash implementation using StableHashBuilder to ensure different
    /// field configurations produce different hashes while maintaining
    /// forward/backward compatibility.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let TargetsV1 {
            default_target,
            targets,
        } = self;

        StableHashBuilder::<H>::new()
            .field("default_target", default_target)
            .field("targets", targets)
            .finish(state);
    }
}

impl Hash for TargetV1 {
    /// Custom hash implementation using StableHashBuilder to ensure different
    /// field configurations produce different hashes while maintaining
    /// forward/backward compatibility.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let TargetV1 {
            build_dependencies,
            host_dependencies,
            run_dependencies,
        } = self;

        StableHashBuilder::<H>::new()
            .field("build_dependencies", build_dependencies)
            .field("host_dependencies", host_dependencies)
            .field("run_dependencies", run_dependencies)
            .finish(state);
    }
}

impl Hash for PackageSpecV1 {
    /// Custom hash implementation that uses discriminant values to keep the
    /// hash as stable as possible when adding new enum variants.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            PackageSpecV1::Binary(spec) => {
                0u8.hash(state);
                spec.hash(state);
            }
            PackageSpecV1::Source(spec) => {
                1u8.hash(state);
                spec.hash(state);
            }
        }
    }
}

impl Hash for SourcePackageSpecV1 {
    /// Custom hash implementation that uses discriminant values to keep the
    /// hash as stable as possible when adding new enum variants.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            SourcePackageSpecV1::Url(spec) => {
                0u8.hash(state);
                spec.hash(state);
            }
            SourcePackageSpecV1::Git(spec) => {
                1u8.hash(state);
                spec.hash(state);
            }
            SourcePackageSpecV1::Path(spec) => {
                2u8.hash(state);
                spec.hash(state);
            }
        }
    }
}

impl Hash for UrlSpecV1 {
    /// Custom hash implementation using StableHashBuilder to ensure different
    /// field configurations produce different hashes while maintaining
    /// forward/backward compatibility.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let UrlSpecV1 { url, md5, sha256 } = self;

        StableHashBuilder::<H>::new()
            .field("md5", md5)
            .field("sha256", sha256)
            .field("url", url)
            .finish(state);
    }
}

impl Hash for GitSpecV1 {
    /// Custom hash implementation using StableHashBuilder to ensure different
    /// field configurations produce different hashes while maintaining
    /// forward/backward compatibility.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        StableHashBuilder::<H>::new()
            .field("git", &self.git)
            .field("rev", &self.rev)
            .field("subdirectory", &self.subdirectory)
            .finish(state);
    }
}

impl Hash for PathSpecV1 {
    /// Custom hash implementation to keep the hash as stable as possible.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let PathSpecV1 { path } = self;

        path.hash(state);
    }
}

impl Hash for GitReferenceV1 {
    /// Custom hash implementation that uses discriminant values to keep the
    /// hash as stable as possible when adding new enum variants.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            GitReferenceV1::Branch(b) => {
                0u8.hash(state);
                b.hash(state);
            }
            GitReferenceV1::Tag(t) => {
                1u8.hash(state);
                t.hash(state);
            }
            GitReferenceV1::Rev(r) => {
                2u8.hash(state);
                r.hash(state);
            }
            GitReferenceV1::DefaultBranch => {
                3u8.hash(state);
            }
        }
    }
}

impl IsDefault for GitReferenceV1 {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self) // Never skip GitReferenceV1 fields
    }
}

impl Hash for BinaryPackageSpecV1 {
    /// Custom hash implementation using StableHashBuilder to ensure different
    /// field configurations produce different hashes while maintaining
    /// forward/backward compatibility.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        StableHashBuilder::<H>::new()
            .field("build", &self.build)
            .field("build_number", &self.build_number)
            .field("channel", &self.channel)
            .field("file_name", &self.file_name)
            .field("license", &self.license)
            .field("md5", &self.md5)
            .field("sha256", &self.sha256)
            .field("subdir", &self.subdir)
            .field("url", &self.url)
            .field("version", &self.version)
            .finish(state);
    }
}

#[cfg(test)]
mod tests {
    use std::hash::{DefaultHasher, Hash, Hasher};

    use super::*;

    fn calculate_hash<T: Hash>(obj: &T) -> u64 {
        let mut hasher = DefaultHasher::default();
        obj.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn test_hash_stability_with_default_values() {
        // Create a minimal ProjectModelV1 instance
        let mut project_model = ProjectModelV1 {
            name: Some("test-project".to_string()),
            version: None,
            description: None,
            authors: None,
            license: None,
            license_file: None,
            readme: None,
            homepage: None,
            repository: None,
            documentation: None,
            targets: None,
        };

        let hash1 = calculate_hash(&project_model);

        // Add empty targets field - with corrected implementation, this should NOT
        // change hash because we only include discriminants for
        // non-default/non-empty values
        project_model.targets = Some(TargetsV1 {
            default_target: None,
            targets: Some(OrderMap::new()),
        });
        let hash2 = calculate_hash(&project_model);

        // Add a target with empty dependencies - this should also NOT change hash
        let empty_target = TargetV1 {
            host_dependencies: Some(OrderMap::new()),
            build_dependencies: Some(OrderMap::new()),
            run_dependencies: Some(OrderMap::new()),
        };
        project_model.targets = Some(TargetsV1 {
            default_target: Some(empty_target),
            targets: Some(OrderMap::new()),
        });
        let hash3 = calculate_hash(&project_model);

        // With corrected implementation, hashes should remain stable when adding
        // empty/default values This preserves forward/backward compatibility
        assert_eq!(
            hash1, hash2,
            "Hash should not change when adding empty targets (maintains forward compatibility)"
        );
        assert_eq!(
            hash1, hash3,
            "Hash should not change when adding empty target with empty dependencies"
        );
        assert_eq!(
            hash2, hash3,
            "Hash should remain stable across different empty configurations"
        );
    }

    #[test]
    fn test_hash_changes_with_meaningful_values() {
        // Create a minimal ProjectModelV1 instance
        let mut project_model = ProjectModelV1 {
            name: Some("test-project".to_string()),
            version: None,
            description: None,
            authors: None,
            license: None,
            license_file: None,
            readme: None,
            homepage: None,
            repository: None,
            documentation: None,
            targets: None,
        };

        let hash1 = calculate_hash(&project_model);

        // Add a meaningful field (should change hash)
        project_model.description = Some("A test project".to_string());
        let hash2 = calculate_hash(&project_model);

        // Add a real dependency (should change hash)
        let mut deps = OrderMap::new();
        deps.insert("python".to_string(), PackageSpecV1::Binary(Box::default()));

        let target_with_deps = TargetV1 {
            host_dependencies: Some(deps),
            build_dependencies: Some(OrderMap::new()),
            run_dependencies: Some(OrderMap::new()),
        };
        project_model.targets = Some(TargetsV1 {
            default_target: Some(target_with_deps),
            targets: Some(OrderMap::new()),
        });
        let hash3 = calculate_hash(&project_model);

        // Hash should change when adding meaningful values
        assert_ne!(hash1, hash2, "Hash should change when adding description");
        assert_ne!(
            hash1, hash3,
            "Hash should change when adding real dependency"
        );
        assert_ne!(
            hash2, hash3,
            "Hash should change when adding dependency to project with description"
        );
    }

    #[test]
    fn test_binary_package_spec_hash_stability() {
        let spec1 = BinaryPackageSpecV1::default();
        let hash1 = calculate_hash(&spec1);

        // Create another default spec with explicit None values
        let spec2 = BinaryPackageSpecV1 {
            version: None,
            build: None,
            build_number: None,
            file_name: None,
            channel: None,
            subdir: None,
            md5: None,
            sha256: None,
            url: None,
            license: None,
        };
        let hash2 = calculate_hash(&spec2);

        // Both should have the same hash since they're effectively the same
        assert_eq!(
            hash1, hash2,
            "Default spec and explicit None spec should have same hash"
        );

        // Add a meaningful value
        let spec3 = BinaryPackageSpecV1 {
            file_name: Some("test.tar.bz2".to_string()),
            ..Default::default()
        };
        let hash3 = calculate_hash(&spec3);

        assert_ne!(
            hash1, hash3,
            "Hash should change when adding meaningful value"
        );
    }

    #[test]
    fn test_enum_variant_hash_stability() {
        // Test PackageSpecV1 enum variants
        let binary_spec = PackageSpecV1::Binary(Box::default());
        let source_spec = PackageSpecV1::Source(SourcePackageSpecV1::Path(PathSpecV1 {
            path: "test".to_string(),
        }));

        let hash1 = calculate_hash(&binary_spec);
        let hash2 = calculate_hash(&source_spec);

        // Different enum variants should have different hashes
        assert_ne!(
            hash1, hash2,
            "Different enum variants should have different hashes"
        );

        // Same variant with same content should have same hash
        let binary_spec2 = PackageSpecV1::Binary(Box::default());
        let hash3 = calculate_hash(&binary_spec2);

        assert_eq!(
            hash1, hash3,
            "Same enum variant with same content should have same hash"
        );
    }

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

    #[test]
    fn test_hash_collision_bug_dependency_fields() {
        // Test that moving dependencies between different dependency types produces
        // different hashes

        let mut deps = OrderMap::new();
        deps.insert("python".to_string(), PackageSpecV1::Binary(Box::default()));

        // Same dependency in host_dependencies
        let target1 = TargetV1 {
            host_dependencies: Some(deps.clone()),
            build_dependencies: None,
            run_dependencies: None,
        };

        // Same dependency in run_dependencies
        let target2 = TargetV1 {
            host_dependencies: None,
            build_dependencies: None,
            run_dependencies: Some(deps.clone()),
        };

        // Same dependency in build_dependencies
        let target3 = TargetV1 {
            host_dependencies: None,
            build_dependencies: Some(deps.clone()),
            run_dependencies: None,
        };

        let hash1 = calculate_hash(&target1);
        let hash2 = calculate_hash(&target2);
        let hash3 = calculate_hash(&target3);

        assert_ne!(
            hash1, hash2,
            "Same dependency in host vs run should produce different hashes"
        );
        assert_ne!(
            hash1, hash3,
            "Same dependency in host vs build should produce different hashes"
        );
        assert_ne!(
            hash2, hash3,
            "Same dependency in run vs build should produce different hashes"
        );

        // Test with TargetsV1 as well
        let targets1 = TargetsV1 {
            default_target: Some(target1),
            targets: None,
        };

        let targets2 = TargetsV1 {
            default_target: Some(target2),
            targets: None,
        };

        let targets_hash1 = calculate_hash(&targets1);
        let targets_hash2 = calculate_hash(&targets2);

        assert_ne!(
            targets_hash1, targets_hash2,
            "TargetsV1 should produce different hashes for different dependency types"
        );
    }

    #[test]
    fn test_hash_collision_bug_project_model() {
        // Test the same issue in ProjectModelV1
        let project1 = ProjectModelV1 {
            name: Some("test".to_string()),
            description: Some("test description".to_string()),
            license: None,
            ..Default::default()
        };

        let project2 = ProjectModelV1 {
            name: Some("test".to_string()),
            description: None,
            license: Some("test description".to_string()),
            ..Default::default()
        };

        let hash1 = calculate_hash(&project1);
        let hash2 = calculate_hash(&project2);

        assert_ne!(
            hash1, hash2,
            "Same value in different fields should produce different hashes in ProjectModelV1"
        );
    }
}
