mod activation;
mod build_system;
pub(crate) mod channel;
mod dependencies;
mod discovery;
mod environment;
mod environments;
mod error;
mod exclude_newer;
mod feature;
mod features_ext;
mod has_features_iter;
mod has_manifest_ref;
mod manifests;
mod package;
mod preview;
pub mod pypi;
pub mod pyproject;
mod s3;
mod solve_group;
mod spec_type;
mod system_requirements;
mod target;
pub mod task;
pub mod toml;
pub mod utils;
mod warning;
mod workspace;
pub use activation::Activation;
pub use build_system::BuildBackend;
pub use build_system::PackageBuild;
pub use channel::PrioritizedChannel;
pub use dependencies::{CondaDependencies, PyPiDependencies};
pub use discovery::{
    DiscoveryStart, ExplicitManifestError, LoadManifestsError, Manifests, WorkspaceDiscoverer,
    WorkspaceDiscoveryError,
};
pub use environment::{Environment, EnvironmentName};
pub use error::TomlError;
pub use feature::{Feature, FeatureName};
pub use features_ext::FeaturesExt;
pub use has_features_iter::HasFeaturesIter;
pub use has_manifest_ref::HasWorkspaceManifest;
use itertools::Itertools;
pub use manifests::{
    AssociateProvenance, ManifestKind, ManifestProvenance, ManifestSource, PackageManifest,
    ProvenanceError, WithProvenance, WorkspaceManifest, WorkspaceManifestMut,
};
use miette::Diagnostic;
pub use package::Package;
pub use preview::{KnownPreviewFeature, Preview};
use rattler_conda_types::Platform;
pub use s3::S3Options;
pub use spec_type::SpecType;
pub use system_requirements::{
    GLIBC_FAMILY, LibCFamilyAndVersion, LibCSystemRequirement, MUSL_FAMILY, SystemRequirements,
};
pub use target::{PackageTarget, TargetSelector, Targets, WorkspaceTarget};
pub use task::{Task, TaskName};
use thiserror::Error;
pub use warning::{Warning, WarningWithSource, WithWarnings};
pub use workspace::{ChannelPriority, Workspace};

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

#[derive(Copy, Clone)]
pub enum PypiDependencyLocation {
    /// [pypi-dependencies] in pixi.toml or [tool.pixi.pypi-dependencies] in
    /// pyproject.toml
    PixiPypiDependencies,
    /// [project.dependencies] in pyproject.toml
    Dependencies,
    /// [project.optional-dependencies] table in pyproject.toml
    OptionalDependencies,
    /// [dependency-groups] in pyproject.toml
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
pub use manifests::ManifestDocument;
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
