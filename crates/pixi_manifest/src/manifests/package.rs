use indexmap::IndexMap;
use pixi_spec::PixiSpec;
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::PackageName;

use crate::target::PackageTarget;
use crate::{PackageBuild, Targets, package::Package};

/// Holds the parsed content of the package part of a pixi manifest. This
/// describes the part related to the package only.
#[derive(Debug, Clone)]
pub struct PackageManifest {
    /// Information about the package
    pub package: Package,

    /// Information about the build system for the package
    pub build: PackageBuild,

    /// Defines the dependencies of the package
    pub targets: Targets<PackageTarget>,

    /// Optional dependency groups declared by the package.
    pub extras: IndexMap<String, DependencyMap<PackageName, PixiSpec>>,
}
