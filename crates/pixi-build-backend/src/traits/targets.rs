//! Targets behaviour traits.
//!
//! # Key components
//!
//! * [`Targets`] - A project target trait.
//! * [`TargetSelector`] - An extension trait that extends the target selector with additional functionality.
//! * [`Dependencies`] - A wrapper struct that contains all dependencies for a target.
use indexmap::IndexMap;
use itertools::{Either, Itertools};
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
        host: IndexMap<&'a SourcePackageName, &'a S>,
        build: IndexMap<&'a SourcePackageName, &'a S>,
    ) -> Self {
        Self { run, host, build }
    }

    /// Return an empty Dependencies
    pub fn empty() -> Self {
        Self {
            run: IndexMap::new(),
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
pub trait Targets {
    /// The selector, in pixi this is something like `[target.linux-64]
    type Selector: TargetSelector;
    /// The target it is resolving to
    type Target;

    /// The Spec type that is used in the package spec
    type Spec: PackageSpec;

    /// Returns the default target.
    fn default_target(&self) -> Option<&Self::Target>;

    /// Return a spec that matches any version
    fn empty_spec() -> Self::Spec;

    /// Returns all targets
    fn targets(&self) -> impl Iterator<Item = (&Self::Selector, &Self::Target)>;

    /// Return all dependencies for the given platform
    fn dependencies(&self, platform: Option<Platform>) -> Dependencies<'_, Self::Spec>;

    /// Return the run dependencies for the given platform
    fn run_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> IndexMap<&SourcePackageName, &Self::Spec>;

    /// Return the host dependencies for the given platform
    fn host_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> IndexMap<&SourcePackageName, &Self::Spec>;

    /// Return the build dependencies for the given platform
    fn build_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> IndexMap<&SourcePackageName, &Self::Spec>;

    /// Resolve the target for the given platform.
    fn resolve(&self, platform: Option<Platform>) -> impl Iterator<Item = &Self::Target> {
        if let Some(platform) = platform {
            let iter = self
                .default_target()
                .into_iter()
                .chain(self.targets().filter_map(move |(selector, target)| {
                    if selector.matches(platform) {
                        Some(target)
                    } else {
                        None
                    }
                }));
            Either::Right(iter)
        } else {
            Either::Left(self.default_target().into_iter())
        }
    }
}

// === Below here are the implementations for v1 ===
impl TargetSelector for pbt::TargetSelector {
    fn matches(&self, platform: Platform) -> bool {
        match self {
            pbt::TargetSelector::Platform(p) => p == &platform.to_string(),
            pbt::TargetSelector::Linux => platform.is_linux(),
            pbt::TargetSelector::Unix => platform.is_unix(),
            pbt::TargetSelector::Win => platform.is_windows(),
            pbt::TargetSelector::MacOs => platform.is_osx(),
        }
    }
}

impl Targets for pbt::Targets {
    type Selector = pbt::TargetSelector;
    type Target = pbt::Target;

    type Spec = pbt::PackageSpec;

    fn default_target(&self) -> Option<&pbt::Target> {
        self.default_target.as_ref()
    }

    fn targets(&self) -> impl Iterator<Item = (&pbt::TargetSelector, &pbt::Target)> {
        self.targets.iter().flatten()
    }

    fn empty_spec() -> pbt::PackageSpec {
        pbt::PackageSpec::Binary(rattler_conda_types::VersionSpec::Any.into())
    }

    fn run_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> IndexMap<&SourcePackageName, &pbt::PackageSpec> {
        let targets = self.resolve(platform).collect_vec();

        targets
            .iter()
            .flat_map(|t| t.run_dependencies.iter())
            .flatten()
            .collect::<IndexMap<&pbt::SourcePackageName, &pbt::PackageSpec>>()
    }

    fn host_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> IndexMap<&SourcePackageName, &pbt::PackageSpec> {
        let targets = self.resolve(platform).collect_vec();

        targets
            .iter()
            .flat_map(|t| t.host_dependencies.iter())
            .flatten()
            .collect::<IndexMap<&pbt::SourcePackageName, &pbt::PackageSpec>>()
    }

    fn build_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> IndexMap<&SourcePackageName, &pbt::PackageSpec> {
        let targets = self.resolve(platform).collect_vec();

        targets
            .iter()
            .flat_map(|t| t.build_dependencies.iter())
            .flatten()
            .collect::<IndexMap<&pbt::SourcePackageName, &pbt::PackageSpec>>()
    }

    fn dependencies(&self, platform: Option<Platform>) -> Dependencies<'_, Self::Spec> {
        let build_deps = self.build_dependencies(platform);
        let host_deps = self.host_dependencies(platform);
        let run_deps = self.run_dependencies(platform);

        Dependencies::new(run_deps, host_deps, build_deps)
    }
}
