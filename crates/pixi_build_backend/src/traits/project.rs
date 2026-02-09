//! Project behaviour traits.
//!
//! # Key components
//!
//! * [`ProjectModel`] - Core trait for project model interface

use std::collections::HashSet;

use itertools::Itertools;
use pixi_build_types::{self as pbt};
use rattler_build::NormalizedKey;
use rattler_conda_types::{Platform, Version};

use super::{Dependencies, PackageSpec, targets::Targets};

/// A trait that defines the project model interface
pub trait ProjectModel {
    /// The targets type of the project model
    type Targets: Targets;

    /// Return the targets of the project model
    fn targets(&self) -> Option<&Self::Targets>;

    /// Return the dependencies of the project model
    fn dependencies(
        &self,
        platform: Option<Platform>,
    ) -> Dependencies<'_, <<Self as ProjectModel>::Targets as Targets>::Spec> {
        self.targets()
            .map(|t| t.dependencies(platform))
            .unwrap_or_default()
    }

    /// Return the used variants of the project model
    fn used_variants(&self, platform: Option<Platform>) -> HashSet<NormalizedKey>;

    /// Return the name of the project model
    fn name(&self) -> Option<&String>;

    /// Return the version of the project model
    fn version(&self) -> &Option<Version>;
}

impl ProjectModel for pbt::ProjectModel {
    type Targets = pbt::Targets;

    fn targets(&self) -> Option<&Self::Targets> {
        self.targets.as_ref()
    }

    fn name(&self) -> Option<&String> {
        self.name.as_ref()
    }

    fn version(&self) -> &Option<Version> {
        &self.version
    }

    fn used_variants(&self, platform: Option<Platform>) -> HashSet<NormalizedKey> {
        let build_dependencies = self
            .targets()
            .iter()
            .flat_map(|target| target.build_dependencies(platform))
            .collect_vec();

        let host_dependencies = self
            .targets()
            .iter()
            .flat_map(|target| target.host_dependencies(platform))
            .collect_vec();

        let run_dependencies = self
            .targets()
            .iter()
            .flat_map(|target| target.run_dependencies(platform))
            .collect_vec();

        build_dependencies
            .iter()
            .chain(host_dependencies.iter())
            .chain(run_dependencies.iter())
            .filter(|(_, spec)| spec.can_be_used_as_variant())
            .map(|(name, _)| name.as_str().into())
            .collect()
    }
}

/// Return a spec of a project model that matches any version
pub fn new_spec<P: ProjectModel>() -> <<P as ProjectModel>::Targets as Targets>::Spec {
    P::Targets::empty_spec()
}
