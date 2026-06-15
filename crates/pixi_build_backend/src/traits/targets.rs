//! Targets behaviour traits.
//!
//! # Key components
//!
//! * [`Targets`] - A project target trait.
//! * [`TargetSelector`] - An extension trait that extends the target selector with additional functionality.
//! * [`Dependencies`] - A wrapper struct that contains all dependencies for a target.
use indexmap::IndexMap;
use itertools::Itertools;
use pixi_build_types::SourcePackageName;
use rattler_conda_types::Platform;

use crate::PackageSpec;
use pixi_build_types::{self as pbt};

/// A trait that extend the target selector with additional functionality.
pub trait TargetSelector {
    /// Does the target selector match the platform?
    fn matches(&self, platform: Platform) -> bool;
}

#[derive(Debug)]
/// A wrapper struct that contains all dependencies for a target
pub struct Dependencies<'a, S> {
    /// The run dependencies
    pub run: IndexMap<&'a SourcePackageName, &'a S>,
    /// The run constraints
    pub run_constraints: IndexMap<&'a SourcePackageName, &'a S>,
    /// The host dependencies
    pub host: IndexMap<&'a SourcePackageName, &'a S>,
    /// The build dependencies
    pub build: IndexMap<&'a SourcePackageName, &'a S>,
}

impl<S> Default for Dependencies<'_, S> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<'a, S> Dependencies<'a, S> {
    /// Create a new Dependencies
    pub fn new(
        run: IndexMap<&'a SourcePackageName, &'a S>,
        run_constraints: IndexMap<&'a SourcePackageName, &'a S>,
        host: IndexMap<&'a SourcePackageName, &'a S>,
        build: IndexMap<&'a SourcePackageName, &'a S>,
    ) -> Self {
        Self {
            run,
            run_constraints,
            host,
            build,
        }
    }

    /// Return an empty Dependencies
    pub fn empty() -> Self {
        Self {
            run: IndexMap::new(),
            run_constraints: IndexMap::new(),
            host: IndexMap::new(),
            build: IndexMap::new(),
        }
    }

    /// Return true if the dependencies contains the given package name
    pub fn contains(&self, name: &SourcePackageName) -> bool {
        self.run.contains_key(name) || self.host.contains_key(name) || self.build.contains_key(name)
    }

    /// Return an iterator of all package names from build and host dependencies.
    /// This is useful for checking build tools and compilers.
    pub fn build_and_host_names(&self) -> impl Iterator<Item = &str> {
        self.build
            .keys()
            .chain(self.host.keys())
            .map(|name| name.as_ref() as &str)
            .unique()
    }
}

/// A trait that represent a project target.
///
/// Dependencies are carried on the default target plus conditional
/// `if(<expression>)` entries. The conditional entries are evaluated by
/// rattler-build, not here, so the dependency accessors expose the default
/// target only.
pub trait Targets {
    /// The target it is resolving to
    type Target;

    /// The Spec type that is used in the package spec
    type Spec: PackageSpec;

    /// Returns the default target.
    fn default_target(&self) -> Option<&Self::Target>;

    /// Return a spec that matches any version
    fn empty_spec() -> Self::Spec;

    /// Return all dependencies of the default target
    fn dependencies(&self) -> Dependencies<'_, Self::Spec>;

    /// Return all dependencies declared under conditional targets, ignoring
    /// the conditions. This is a may-use over-approximation: an entry is
    /// included even when its condition never evaluates to true for a
    /// particular build.
    fn conditional_dependencies(&self) -> Dependencies<'_, Self::Spec>;

    /// Return the run dependencies of the default target
    fn run_dependencies(&self) -> IndexMap<&SourcePackageName, &Self::Spec>;

    /// Return the run constraints of the default target
    fn run_constraints(&self) -> IndexMap<&SourcePackageName, &Self::Spec>;

    /// Return the host dependencies of the default target
    fn host_dependencies(&self) -> IndexMap<&SourcePackageName, &Self::Spec>;

    /// Return the build dependencies of the default target
    fn build_dependencies(&self) -> IndexMap<&SourcePackageName, &Self::Spec>;
}

// === Below here are the implementations for v1 ===
impl TargetSelector for pbt::TargetSelector {
    fn matches(&self, platform: Platform) -> bool {
        match self {
            pbt::TargetSelector::Platform(p) => p == &platform.to_string(),
            pbt::TargetSelector::Subdir(s) => s == &platform.to_string(),
            pbt::TargetSelector::Linux => platform.is_linux(),
            pbt::TargetSelector::Unix => platform.is_unix(),
            pbt::TargetSelector::Win => platform.is_windows(),
            pbt::TargetSelector::MacOs => platform.is_osx(),
        }
    }
}

impl Targets for pbt::Targets {
    type Target = pbt::Target;

    type Spec = pbt::PackageSpec;

    fn default_target(&self) -> Option<&pbt::Target> {
        self.default_target.as_ref()
    }

    fn empty_spec() -> pbt::PackageSpec {
        rattler_conda_types::VersionSpec::Any.into()
    }

    fn run_dependencies(&self) -> IndexMap<&SourcePackageName, &pbt::PackageSpec> {
        self.default_target()
            .into_iter()
            .flat_map(|t| t.run_dependencies.iter())
            .flatten()
            .collect::<IndexMap<&pbt::SourcePackageName, &pbt::PackageSpec>>()
    }

    fn run_constraints(&self) -> IndexMap<&SourcePackageName, &pbt::PackageSpec> {
        self.default_target()
            .into_iter()
            .flat_map(|t| t.run_constraints.iter())
            .flatten()
            .collect::<IndexMap<&pbt::SourcePackageName, &pbt::PackageSpec>>()
    }

    fn host_dependencies(&self) -> IndexMap<&SourcePackageName, &pbt::PackageSpec> {
        self.default_target()
            .into_iter()
            .flat_map(|t| t.host_dependencies.iter())
            .flatten()
            .collect::<IndexMap<&pbt::SourcePackageName, &pbt::PackageSpec>>()
    }

    fn build_dependencies(&self) -> IndexMap<&SourcePackageName, &pbt::PackageSpec> {
        self.default_target()
            .into_iter()
            .flat_map(|t| t.build_dependencies.iter())
            .flatten()
            .collect::<IndexMap<&pbt::SourcePackageName, &pbt::PackageSpec>>()
    }

    fn dependencies(&self) -> Dependencies<'_, Self::Spec> {
        let build_deps = self.build_dependencies();
        let host_deps = self.host_dependencies();
        let run_deps = self.run_dependencies();
        let run_constraints = self.run_constraints();

        Dependencies::new(run_deps, run_constraints, host_deps, build_deps)
    }

    fn conditional_dependencies(&self) -> Dependencies<'_, Self::Spec> {
        let conditional_targets = || self.conditional.iter().flatten().map(|(_, target)| target);

        let run = conditional_targets()
            .flat_map(|target| target.run_dependencies.iter().flatten())
            .collect();
        let run_constraints = conditional_targets()
            .flat_map(|target| target.run_constraints.iter().flatten())
            .collect();
        let host = conditional_targets()
            .flat_map(|target| target.host_dependencies.iter().flatten())
            .collect();
        let build = conditional_targets()
            .flat_map(|target| target.build_dependencies.iter().flatten())
            .collect();

        Dependencies::new(run, run_constraints, host, build)
    }
}
