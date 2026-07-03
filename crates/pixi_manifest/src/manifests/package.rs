use std::hash::{Hash, Hasher};

use indexmap::IndexMap;
use pixi_build_types::ConditionalExpression;
use rattler_conda_types::PackageName;

use crate::target::{InlinePackageManifest, PackageTarget};
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

impl PackageManifest {
    /// Returns the inline package definitions declared by this package's
    /// dependency tables, merged across the default and conditional targets.
    ///
    /// Which conditional targets apply is decided by the build backend, so the
    /// merge is name-keyed and target-agnostic; parsing rejects the ambiguous
    /// case of a name carrying *different* definitions in different targets.
    pub fn combined_inline_packages(&self) -> IndexMap<PackageName, &InlinePackageManifest> {
        let mut merged: IndexMap<PackageName, &InlinePackageManifest> = IndexMap::new();
        for target in
            std::iter::once(&self.dependencies).chain(self.conditional_dependencies.values())
        {
            for (name, inline) in &target.inline_packages {
                merged.entry(name.clone()).or_insert(inline);
            }
        }
        merged
    }
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
