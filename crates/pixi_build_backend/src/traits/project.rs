//! Project behaviour traits.
//!
//! # Key components
//!
//! * [`ProjectModel`] - Core trait for project model interface

use pixi_build_types::{self as pbt};
use rattler_conda_types::Version;

use super::targets::Targets;

/// A trait that defines the project model interface
pub trait ProjectModel {
    /// The targets type of the project model
    type Targets: Targets;

    /// Return the targets of the project model
    fn targets(&self) -> Option<&Self::Targets>;

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
}

/// Return a spec of a project model that matches any version
pub fn new_spec<P: ProjectModel>() -> <<P as ProjectModel>::Targets as Targets>::Spec {
    P::Targets::empty_spec()
}
