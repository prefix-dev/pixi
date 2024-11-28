use crate::target::PackageTarget;
use crate::{package::Package, BuildSystem, Targets};

/// Holds the parsed content of the package part of a pixi manifest. This
/// describes the part related to the package only.
#[derive(Debug, Clone)]
pub struct PackageManifest {
    /// Information about the package
    pub package: Package,

    /// Information about the build system for the package
    pub build_system: BuildSystem,

    /// Defines the dependencies of the package
    pub targets: Targets<PackageTarget>,
}
