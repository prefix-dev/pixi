//! This module is a collection of types that represent a pixi package in a
//! protocol format that can be sent over the wire.
//! We need to vendor a lot of the types and simplify them in some cases so
//! that we have a stable protocol that can be used to communicate in the build
//! tasks.
//!
//! The rationale is that we want to have a stable protocol to provide forwards
//! and backwards compatibility. The idea for **backwards compatibility** is
//! that we try not to break this in pixi as much as possible. So as long as
//! older pixi TOMLs keep loading, we can send them to the backend.
use std::{convert::Infallible, fmt::Display, hash::Hash, path::PathBuf, str::FromStr};
use std::hash::Hasher;
use ordermap::OrderMap;
use pixi_stable_hash::{IsDefault, StableHashBuilder};
use rattler_conda_types::{BuildNumber, BuildNumberSpec, StringMatcher, Version, VersionSpec};
use rattler_digest::{Md5, Md5Hash, Sha256, Sha256Hash, serde::SerializableHash};
use serde::{Deserialize, Serialize, Serializer};
use serde_with::{DeserializeFromStr, DisplayFromStr, SerializeDisplay, serde_as};
use url::Url;

/// The source package name of a package. Not normalized per se.
pub type SourcePackageName = String;

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ProjectModel {
    /// The name of the project
    pub name: Option<String>,

    /// A build string configured by the user.
    pub build_string: Option<String>,

    /// The build number configured by the user.
    #[cfg_attr(feature = "schemars", schemars(with = "Option<u64>"))]
    pub build_number: Option<BuildNumber>,

    /// The version of the project
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
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
    pub targets: Option<Targets>,
}

impl IsDefault for ProjectModel {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self)
    }
}

/// Represents a target selector. Currently, we only support explicit platform
/// selection.
#[derive(Debug, Clone, DeserializeFromStr, SerializeDisplay, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum TargetSelector {
    // Platform specific configuration
    Unix,
    Linux,
    Win,
    MacOs,
    Platform(String),
    // TODO: Add minijinja coolness here.
}

impl Display for TargetSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetSelector::Unix => write!(f, "unix"),
            TargetSelector::Linux => write!(f, "linux"),
            TargetSelector::Win => write!(f, "win"),
            TargetSelector::MacOs => write!(f, "macos"),
            TargetSelector::Platform(p) => write!(f, "{p}"),
        }
    }
}
impl FromStr for TargetSelector {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "unix" => Ok(TargetSelector::Unix),
            "linux" => Ok(TargetSelector::Linux),
            "win" => Ok(TargetSelector::Win),
            "macos" => Ok(TargetSelector::MacOs),
            _ => Ok(TargetSelector::Platform(s.to_string())),
        }
    }
}

/// A collect of targets including a default target.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Targets {
    pub default_target: Option<Target>,

    /// We use an [`OrderMap`] to preserve the order in which the items where
    /// defined in the manifest.
    #[cfg_attr(
        feature = "schemars",
        schemars(with = "Option<std::collections::HashMap<TargetSelector, Target>>")
    )]
    pub targets: Option<OrderMap<TargetSelector, Target>>,
}

impl Targets {
    /// Check if this targets struct is effectively empty (contains no
    /// meaningful data that should affect the hash).
    pub fn is_empty(&self) -> bool {
        let has_meaningless_default_target =
            self.default_target.as_ref().is_none_or(|t| t.is_empty());
        let has_only_empty_targets = self.targets.as_ref().is_none_or(|t| t.is_empty());

        has_meaningless_default_target && has_only_empty_targets
    }
}

impl IsDefault for Targets {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        if !self.is_empty() { Some(self) } else { None }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Target {
    /// Host dependencies of the project
    #[cfg_attr(
        feature = "schemars",
        schemars(with = "Option<std::collections::HashMap<SourcePackageName, PackageSpec>>")
    )]
    pub host_dependencies: Option<OrderMap<SourcePackageName, PackageSpec>>,

    /// Build dependencies of the project
    #[cfg_attr(
        feature = "schemars",
        schemars(with = "Option<std::collections::HashMap<SourcePackageName, PackageSpec>>")
    )]
    pub build_dependencies: Option<OrderMap<SourcePackageName, PackageSpec>>,

    /// Run dependencies of the project
    #[cfg_attr(
        feature = "schemars",
        schemars(with = "Option<std::collections::HashMap<SourcePackageName, PackageSpec>>")
    )]
    pub run_dependencies: Option<OrderMap<SourcePackageName, PackageSpec>>,
}

impl Target {
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

impl IsDefault for Target {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        if !self.is_empty() { Some(self) } else { None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum PackageSpec {
    /// This is a binary dependency
    Binary(BinaryPackageSpec),
    /// This is a dependency on a source package
    Source(SourcePackageSpec),
    /// Pin to a version that is compatible with a version from the "previous" environment
    PinCompatible(PinCompatibleSpec),
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct PinCompatibleSpec {
    /// A minimum pin to a version, using `x.x.x...` as syntax
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_bound: Option<PinBound>,

    /// A pin to a version, using `x.x.x...` as syntax
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upper_bound: Option<PinBound>,

    /// If an exact pin is given, we pin the exact version & hash
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exact: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum PinBound {
    Expression(String),
    Version(
        #[cfg_attr(feature = "schemars", schemars(with = "String"))]
        Version,
    ),
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct NamedSpec<T> {
    pub name: SourcePackageName,

    #[serde(flatten)]
    pub spec: T,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SourcePackageSpec {
    #[serde(flatten)]
    pub location: SourcePackageLocationSpec,
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub version: Option<VersionSpec>,
    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub build: Option<StringMatcher>,
    /// The build number of the package
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub build_number: Option<BuildNumberSpec>,
    /// The subdir of the channel
    pub subdir: Option<String>,
    /// The md5 hash of the package
    /// The license of the package
    pub license: Option<String>,
}

impl From<PathSpec> for SourcePackageSpec {
    fn from(value: PathSpec) -> Self {
        Self {
            location: SourcePackageLocationSpec::Path(value),
            version: None,
            build: None,
            build_number: None,
            subdir: None,
            license: None,
        }
    }
}

impl From<UrlSpec> for SourcePackageSpec {
    fn from(value: UrlSpec) -> Self {
        Self {
            location: SourcePackageLocationSpec::Url(value),
            version: None,
            build: None,
            build_number: None,
            subdir: None,
            license: None,
        }
    }
}

impl From<GitSpec> for SourcePackageSpec {
    fn from(value: GitSpec) -> Self {
        Self {
            location: SourcePackageLocationSpec::Git(value),
            version: None,
            build: None,
            build_number: None,
            subdir: None,
            license: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum SourcePackageLocationSpec {
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

#[serde_as]
#[derive(Clone, Serialize, Deserialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UrlSpec {
    /// The URL of the package
    pub url: Url,

    /// The md5 hash of the package
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Md5>>")]
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the package
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Sha256>>")]
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub sha256: Option<Sha256Hash>,

    /// The subdirectory of the package in the archive
    pub subdirectory: Option<String>,
}

impl std::fmt::Debug for UrlSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug_struct = f.debug_struct("UrlSpec");

        debug_struct.field("url", &self.url);
        if let Some(md5) = &self.md5 {
            debug_struct.field("md5", &format!("{md5:x}"));
        }
        if let Some(sha256) = &self.sha256 {
            debug_struct.field("sha256", &format!("{sha256:x}"));
        }
        debug_struct.finish()
    }
}

/// A specification of a package from a git repository.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct GitSpec {
    /// The git url of the package which can contain git+ prefixes.
    pub git: Url,

    /// The git revision of the package
    pub rev: Option<GitReference>,

    /// The git subdirectory of the package
    pub subdirectory: Option<String>,
}

/// A specification of a package from a path
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct PathSpec {
    /// The path to the package
    pub path: String,
}

/// A reference to a specific commit in a git repository.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum GitReference {
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
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct BinaryPackageSpec {
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub version: Option<VersionSpec>,
    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub build: Option<StringMatcher>,
    /// The build number of the package
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub build_number: Option<BuildNumberSpec>,
    /// Match the specific filename of the package
    pub file_name: Option<String>,
    /// The channel of the package
    pub channel: Option<Url>,
    /// The subdir of the channel
    pub subdir: Option<String>,
    /// The md5 hash of the package
    #[serde_as(as = "Option<SerializableHash<Md5>>")]
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub md5: Option<Md5Hash>,
    /// The sha256 hash of the package
    #[serde_as(as = "Option<SerializableHash<Sha256>>")]
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub sha256: Option<Sha256Hash>,
    /// The URL of the package, if it is available
    pub url: Option<Url>,
    /// The license of the package
    pub license: Option<String>,
}

impl From<VersionSpec> for BinaryPackageSpec {
    fn from(value: VersionSpec) -> Self {
        Self {
            version: Some(value),
            ..Default::default()
        }
    }
}

impl From<&VersionSpec> for BinaryPackageSpec {
    fn from(value: &VersionSpec) -> Self {
        Self {
            version: Some(value.clone()),
            ..Default::default()
        }
    }
}

impl std::fmt::Debug for BinaryPackageSpec {
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
            debug_struct.field("md5", &format!("{md5:x}"));
        }
        if let Some(sha256) = &self.sha256 {
            debug_struct.field("sha256", &format!("{sha256:x}"));
        }

        debug_struct.finish()
    }
}

// Custom Hash implementations that skip default values for stability
impl Hash for ProjectModel {
    /// Custom hash implementation using StableHashBuilder to ensure different
    /// field configurations produce different hashes while maintaining
    /// forward/backward compatibility.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let ProjectModel {
            name,
            build_string,
            build_number,
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
            .field("build_string", build_string)
            .field("build_number", build_number)
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

impl Hash for TargetSelector {
    /// Custom hash implementation that uses discriminant values to keep the
    /// hash as stable as possible when adding new enum variants.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            TargetSelector::Unix => 0u8.hash(state),
            TargetSelector::Linux => 1u8.hash(state),
            TargetSelector::Win => 2u8.hash(state),
            TargetSelector::MacOs => 3u8.hash(state),
            TargetSelector::Platform(p) => {
                4u8.hash(state);
                p.hash(state);
            }
        }
    }
}

impl Hash for Targets {
    /// Custom hash implementation using StableHashBuilder to ensure different
    /// field configurations produce different hashes while maintaining
    /// forward/backward compatibility.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let Targets {
            default_target,
            targets,
        } = self;

        StableHashBuilder::<H>::new()
            .field("default_target", default_target)
            .field("targets", targets)
            .finish(state);
    }
}

impl Hash for Target {
    /// Custom hash implementation using StableHashBuilder to ensure different
    /// field configurations produce different hashes while maintaining
    /// forward/backward compatibility.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let Target {
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

impl Hash for PackageSpec {
    /// Custom hash implementation that uses discriminant values to keep the
    /// hash as stable as possible when adding new enum variants.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            PackageSpec::Binary(spec) => {
                0u8.hash(state);
                spec.hash(state);
            }
            PackageSpec::Source(spec) => {
                1u8.hash(state);
                spec.hash(state);
            }
            PackageSpec::PinCompatible(spec) => {
                2u8.hash(state);
                spec.hash(state);
            }
        }
    }
}

impl Hash for PinCompatibleSpec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let PinCompatibleSpec { lower_bound, upper_bound, exact, build } = self;

        StableHashBuilder::<H>::new()
            .field("lower_bound", lower_bound)
            .field("upper_bound", upper_bound)
            .field("exact", exact)
            .field("build", build)
            .finish(state);
    }
}

impl Hash for PinBound {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            PinBound::Expression(expr) => {
                0u8.hash(state);
                expr.hash(state);
            }
            PinBound::Version(ver) => {
                1u8.hash(state);
                ver.hash(state);
            }
        }
    }
}

impl IsDefault for PinBound {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self)
    }
}

impl Hash for SourcePackageSpec {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let Self {
            location,
            version,
            build,
            build_number,
            subdir,
            license,
        } = self;

        // Hash the location first to ensure compatibility with older versions.
        location.hash(state);

        // Add the new fields using StableHashBuilder for forward/backward
        // compatibility.
        StableHashBuilder::<H>::new()
            .field("build", build)
            .field("build_number", build_number)
            .field("license", license)
            .field("subdir", subdir)
            .field("version", version)
            .finish(state);
    }
}

impl Hash for SourcePackageLocationSpec {
    /// Custom hash implementation that uses discriminant values to keep the
    /// hash as stable as possible when adding new enum variants.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::Url(spec) => {
                0u8.hash(state);
                spec.hash(state);
            }
            Self::Git(spec) => {
                1u8.hash(state);
                spec.hash(state);
            }
            Self::Path(spec) => {
                2u8.hash(state);
                spec.hash(state);
            }
        }
    }
}

impl Hash for UrlSpec {
    /// Custom hash implementation using StableHashBuilder to ensure different
    /// field configurations produce different hashes while maintaining
    /// forward/backward compatibility.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let UrlSpec {
            url,
            md5,
            sha256,
            subdirectory,
        } = self;

        StableHashBuilder::<H>::new()
            .field("md5", md5)
            .field("sha256", sha256)
            .field("url", url)
            .field("subdirectory", subdirectory)
            .finish(state);
    }
}

impl Hash for GitSpec {
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

impl Hash for PathSpec {
    /// Custom hash implementation to keep the hash as stable as possible.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let PathSpec { path } = self;

        path.hash(state);
    }
}

impl Hash for GitReference {
    /// Custom hash implementation that uses discriminant values to keep the
    /// hash as stable as possible when adding new enum variants.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            GitReference::Branch(b) => {
                0u8.hash(state);
                b.hash(state);
            }
            GitReference::Tag(t) => {
                1u8.hash(state);
                t.hash(state);
            }
            GitReference::Rev(r) => {
                2u8.hash(state);
                r.hash(state);
            }
            GitReference::DefaultBranch => {
                3u8.hash(state);
            }
        }
    }
}

impl IsDefault for GitReference {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self) // Never skip GitReferenceV1 fields
    }
}

impl Hash for BinaryPackageSpec {
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
        let mut project_model = ProjectModel {
            name: Some("test-project".to_string()),
            build_number: None,
            build_string: None,
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

        // Add an empty targets field - with corrected implementation, this should NOT
        // change hash because we only include discriminants for
        // non-default/non-empty values
        project_model.targets = Some(Targets {
            default_target: None,
            targets: Some(OrderMap::new()),
        });
        let hash2 = calculate_hash(&project_model);

        // Add a target with empty dependencies - this should also NOT change the hash
        let empty_target = Target {
            host_dependencies: Some(OrderMap::new()),
            build_dependencies: Some(OrderMap::new()),
            run_dependencies: Some(OrderMap::new()),
        };
        project_model.targets = Some(Targets {
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
        let mut project_model = ProjectModel {
            name: Some("test-project".to_string()),
            build_number: None,
            build_string: None,
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
        deps.insert(
            "python".to_string(),
            PackageSpec::Binary(BinaryPackageSpec::default()),
        );

        let target_with_deps = Target {
            host_dependencies: Some(deps),
            build_dependencies: Some(OrderMap::new()),
            run_dependencies: Some(OrderMap::new()),
        };
        project_model.targets = Some(Targets {
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
        let spec1 = BinaryPackageSpec::default();
        let hash1 = calculate_hash(&spec1);

        // Create another default spec with explicit None values
        let spec2 = BinaryPackageSpec {
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
        let spec3 = BinaryPackageSpec {
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
        let binary_spec = PackageSpec::Binary(BinaryPackageSpec::default());
        let source_spec = PackageSpec::Source(SourcePackageSpec::from(PathSpec {
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
        let binary_spec2 = PackageSpec::Binary(BinaryPackageSpec::default());
        let hash3 = calculate_hash(&binary_spec2);

        assert_eq!(
            hash1, hash3,
            "Same enum variant with same content should have same hash"
        );
    }

    fn create_sample_target_v1() -> Target {
        Target {
            host_dependencies: Some(OrderMap::from([(
                "host_dep1".to_string(),
                PackageSpec::Binary(BinaryPackageSpec::default()),
            )])),
            build_dependencies: Some(OrderMap::from([(
                "build_dep1".to_string(),
                PackageSpec::Binary(BinaryPackageSpec::default()),
            )])),
            run_dependencies: Some(OrderMap::from([(
                "run_dep1".to_string(),
                PackageSpec::Binary(BinaryPackageSpec::default()),
            )])),
        }
    }

    #[test]
    fn serialize_targets_v1_with_default_target() {
        let targets = Targets {
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

        let targets = Targets {
            default_target: None,
            targets: Some(
                platform_strs
                    .iter()
                    .map(|s| {
                        let selector = match *s {
                            "unix" => TargetSelector::Unix,
                            "win" => TargetSelector::Win,
                            "macos" => TargetSelector::MacOs,
                            other => TargetSelector::Platform(other.to_string()),
                        };
                        (selector, create_sample_target_v1())
                    })
                    .collect(),
            ),
        };

        let serialized = serde_json::to_string(&targets).unwrap();

        for platform in platform_strs {
            assert!(serialized.contains(platform), "Missing: {platform}");
        }
    }

    #[test]
    fn deserialize_targets_v1_with_empty_fields() {
        let json = r#"{
            "defaultTarget": null,
            "targets": null
        }"#;

        let deserialized: Targets = serde_json::from_str(json).unwrap();
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

        let deserialized: Targets = serde_json::from_str(json).unwrap();
        assert!(deserialized.default_target.is_some());
        assert!(deserialized.targets.is_some());
        assert!(
            deserialized
                .targets
                .unwrap()
                .contains_key(&TargetSelector::Unix)
        );
    }

    #[test]
    fn test_hash_collision_bug_dependency_fields() {
        // Test that moving dependencies between different dependency types produces
        // different hashes

        let mut deps = OrderMap::new();
        deps.insert(
            "python".to_string(),
            PackageSpec::Binary(BinaryPackageSpec::default()),
        );

        // Same dependency in host_dependencies
        let target1 = Target {
            host_dependencies: Some(deps.clone()),
            build_dependencies: None,
            run_dependencies: None,
        };

        // Same dependency in run_dependencies
        let target2 = Target {
            host_dependencies: None,
            build_dependencies: None,
            run_dependencies: Some(deps.clone()),
        };

        // Same dependency in build_dependencies
        let target3 = Target {
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
        let targets1 = Targets {
            default_target: Some(target1),
            targets: None,
        };

        let targets2 = Targets {
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
        let project1 = ProjectModel {
            name: Some("test".to_string()),
            description: Some("test description".to_string()),
            license: None,
            ..Default::default()
        };

        let project2 = ProjectModel {
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
