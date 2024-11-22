mod activation;
mod build_system;
pub(crate) mod channel;
mod dependencies;
mod environment;
mod environments;
mod error;
mod feature;
mod features_ext;
mod has_environment_dependencies;
mod has_features_iter;
mod has_manifest_ref;
mod manifests;
mod preview;
pub mod pypi;
pub mod pyproject;
mod solve_group;
mod spec_type;
mod system_requirements;
mod target;
pub mod task;
pub mod toml;
pub mod utils;
mod validation;
mod workspace;

pub use activation::Activation;
pub use build_system::BuildSystem;
pub use channel::PrioritizedChannel;
pub use dependencies::{CondaDependencies, Dependencies, PyPiDependencies};
pub use environment::{Environment, EnvironmentName};
pub use error::TomlError;
pub use feature::{Feature, FeatureName};
pub use features_ext::FeaturesExt;
pub use has_environment_dependencies::HasEnvironmentDependencies;
pub use has_features_iter::HasFeaturesIter;
pub use has_manifest_ref::HasManifestRef;
use itertools::Itertools;
pub use manifests::{Manifest, ManifestKind, WorkspaceManifest};
use miette::Diagnostic;
pub use preview::{KnownPreviewFeature, Preview, PreviewFeature};
pub use pypi::pypi_requirement::PyPiRequirement;
use rattler_conda_types::Platform;
pub use spec_type::SpecType;
pub use system_requirements::{LibCSystemRequirement, SystemRequirements};
pub use target::{Target, TargetSelector, Targets};
pub use task::{Task, TaskName};
use thiserror::Error;
pub use workspace::Workspace;

pub use crate::{
    environments::Environments,
    solve_group::{SolveGroup, SolveGroups},
};

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

pub enum PypiDependencyLocation {
    // The [pypi-dependencies] or [tool.pixi.pypi-dependencies] table
    Pixi,
    // The [project.optional-dependencies] table in a 'pyproject.toml' manifest
    OptionalDependencies,
    // The [dependency-groups] table in a 'pyproject.toml' manifest
    DependencyGroups,
}

/// Converts an array of Platforms to a non-empty Vec of Option<Platform>
fn to_options(platforms: &[Platform]) -> Vec<Option<Platform>> {
    match platforms.is_empty() {
        true => vec![None],
        false => platforms.iter().map(|p| Some(*p)).collect_vec(),
    }
}

use console::StyledObject;
use fancy_display::FancyDisplay;
use pixi_consts::consts;

impl FancyDisplay for EnvironmentName {
    fn fancy_display(&self) -> StyledObject<&str> {
        consts::ENVIRONMENT_STYLE.apply_to(self.as_str())
    }
}

impl FancyDisplay for &EnvironmentName {
    fn fancy_display(&self) -> StyledObject<&str> {
        consts::ENVIRONMENT_STYLE.apply_to(self.as_str())
    }
}

impl FancyDisplay for TaskName {
    fn fancy_display(&self) -> StyledObject<&str> {
        consts::TASK_STYLE.apply_to(self.as_str())
    }
}

impl FancyDisplay for FeatureName {
    fn fancy_display(&self) -> StyledObject<&str> {
        consts::FEATURE_STYLE.apply_to(self.as_str())
    }
}
