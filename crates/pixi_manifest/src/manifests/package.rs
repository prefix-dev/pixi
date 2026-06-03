use indexmap::IndexMap;
use pixi_build_types::ConditionalExpression;

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

    /// Defines the platform-specific dependencies of the package.
    ///
    /// # Deprecated
    ///
    /// These come from the deprecated `[package.target.<platform>]` tables and
    /// will be removed in a future version. Use [`Self::conditional_dependencies`]
    /// (`if(<expression>)` dependencies) instead.
    pub targets: Targets<PackageTarget>,

    /// Dependencies guarded by an `if(<expression>)` conditional. These are not
    /// platform selectors; the expression is passed through to rattler-build,
    /// which decides whether the dependencies apply.
    pub conditional_dependencies: IndexMap<ConditionalExpression, PackageTarget>,
}
