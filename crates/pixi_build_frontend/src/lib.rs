mod build_frontend;
mod conda_build;
mod jsonrpc;
pub mod pixi;
pub mod protocol;
mod protocol_builder;
mod tool;

use std::path::PathBuf;

pub use crate::protocol::Protocol;
pub use build_frontend::{BuildFrontend, BuildFrontendError};
use pixi_manifest::Dependencies;
use pixi_spec::PixiSpec;
use rattler_conda_types::{MatchSpec, NoArchType, PackageName, Platform, VersionWithSource};
pub use tool::{IsolatedToolSpec, SystemToolSpec, ToolSpec};
use url::Url;

#[derive(Debug, Clone)]
pub struct BackendOverrides {
    /// The specs to use for the build tool.
    pub spec: Option<MatchSpec>,

    /// Path to a system build tool.
    pub path: Option<PathBuf>,
}

#[derive(Debug)]
pub struct SetupRequest {
    /// The source directory that contains the source package.
    pub source_dir: PathBuf,

    /// Overrides for the build tool.
    pub build_tool_overrides: BackendOverrides,
}

#[derive(Debug)]
pub struct BuildOutput {
    /// Paths to the built artifacts.
    pub artifacts: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct CondaMetadata {
    /// Metadata of all the package that can be built from the source directory.
    pub packages: Vec<CondaPackageMetadata>,
}

#[derive(Debug)]
pub struct CondaPackageMetadata {
    /// The name of the package
    pub name: PackageName,

    /// The version of the package
    pub version: VersionWithSource,

    /// The build hash of the package
    pub build: String,

    /// The build number of the package
    pub build_number: u64,

    /// The subdirectory the package would be placed in
    pub subdir: Platform,

    /// The dependencies of the package
    pub depends: Dependencies<PackageName, PixiSpec>,

    /// Constraints of the package
    pub constraints: Dependencies<PackageName, PixiSpec>,

    /// The license of the package
    pub license: Option<String>,

    /// The license family of the package
    pub license_family: Option<String>,

    /// Whether this is a noarch package
    pub noarch: NoArchType,
}

#[derive(Debug)]
pub struct CondaMetadataRequest {
    /// The base urls of the channels to use.
    pub channel_base_urls: Vec<Url>,
}
