mod activation;
pub(crate) mod channel;
pub mod consts;
mod document;
mod environment;
mod environments;
mod error;
mod feature;
mod manifest;
mod metadata;
mod nameless_matchspec;
mod parsed_manifest;
pub mod pypi;
pub mod pyproject;
mod solve_group;
mod spec_type;
mod system_requirements;
mod target;
pub mod task;
mod utils;
mod validation;

pub use manifest::{Manifest, ManifestKind};

pub use crate::environments::Environments;
pub use crate::parsed_manifest::ParsedManifest;
pub use crate::solve_group::{SolveGroup, SolveGroups};
pub use activation::Activation;
pub use channel::PrioritizedChannel;
pub use environment::{Environment, EnvironmentName};
pub use feature::{Feature, FeatureName};
use itertools::Itertools;
pub use metadata::ProjectMetadata;
use miette::Diagnostic;
pub use pypi::pypi_requirement::PyPiRequirement;
use rattler_conda_types::Platform;
pub use spec_type::SpecType;
pub use system_requirements::{LibCSystemRequirement, SystemRequirements};
pub use target::{Target, TargetSelector, Targets};
pub use task::{Task, TaskName};
use thiserror::Error;

/// Errors that can occur when getting a feature.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum GetFeatureError {
    #[error("feature `{0}` does not exist")]
    FeatureDoesNotExist(FeatureName),
}

#[derive(Debug, Copy, Clone)]
pub enum DependencyOverwriteBehavior {
    /// Overwrite anything that is already present.
    Overwrite,

    /// Overwrite only if the dependency is explicitly defined (e.g. it has some
    /// constraints).
    OverwriteIfExplicit,

    /// Ignore any duplicate
    IgnoreDuplicate,

    /// Error on duplicate
    Error,
}

/// Converts an array of Platforms to a non-empty Vec of Option<Platform>
fn to_options(platforms: &[Platform]) -> Vec<Option<Platform>> {
    match platforms.is_empty() {
        true => vec![None],
        false => platforms.iter().map(|p| Some(*p)).collect_vec(),
    }
}
