use std::hash::{Hash, Hasher};

use indexmap::IndexMap;
use pixi_build_types::ConditionalExpression;

use crate::target::PackageTarget;
use crate::{PackageBuild, package::Package};

/// Holds the parsed content of the package part of a pixi manifest. This
/// describes the part related to the package only.
#[derive(Debug, Clone)]
pub struct PackageManifest {
    /// Information about the package
    pub package: Package,

    /// Information about the build system for the package
    pub build: PackageBuild,

    /// The unconditional dependencies of the package.
    pub dependencies: PackageTarget,

    /// Dependencies guarded by an `if(<expression>)` conditional. These are not
    /// platform selectors; the expression is passed through to rattler-build,
    /// which decides whether the dependencies apply. The deprecated
    /// `[package.target.<platform>]` tables are lowered into entries of this
    /// map at parse time.
    pub conditional_dependencies: IndexMap<ConditionalExpression, PackageTarget>,
}

impl Hash for PackageManifest {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.package.hash(state);
        self.build.hash(state);
        self.dependencies.hash(state);
        // `conditional_dependencies` is an `IndexMap`; its declaration order is
        // stable, so hash its entries in order.
        self.conditional_dependencies.len().hash(state);
        for (expression, target) in &self.conditional_dependencies {
            expression.hash(state);
            target.hash(state);
        }
    }
}
