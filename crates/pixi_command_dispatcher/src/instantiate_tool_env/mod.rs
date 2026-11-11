use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::Arc,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD, prelude::BASE64_URL_SAFE_NO_PAD};
use itertools::Itertools;
use miette::Diagnostic;
use pixi_build_types::{PIXI_BUILD_API_VERSION_NAME, PixiBuildApiVersion};
use pixi_record::VariantValue;
use pixi_spec::{BinarySpec, PixiSpec, ResolvedExcludeNewer};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{ChannelUrl, PackageName, VersionWithSource, prefix::Prefix};
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

use crate::{
    BuildEnvironment, SolvePixiEnvironmentError, install_pixi::InstallPixiEnvironmentError,
};

/// Specification for a tool environment. Tool environments are cached between
/// runs.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstantiateToolEnvironmentSpec {
    /// The main requirement of the tool environment.
    pub requirement: (rattler_conda_types::PackageName, PixiSpec),

    /// The requirements of the tool environment.
    #[serde(skip_serializing_if = "DependencyMap::is_empty")]
    pub additional_requirements: DependencyMap<rattler_conda_types::PackageName, PixiSpec>,

    /// Additional constraints applied to the environment.
    #[serde(skip_serializing_if = "DependencyMap::is_empty")]
    pub constraints: DependencyMap<rattler_conda_types::PackageName, BinarySpec>,

    /// The platform to instantiate the tool environment for.
    pub build_environment: BuildEnvironment,

    /// The channels to use for solving
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ChannelUrl>,

    /// Exclude packages newer than the configured cutoffs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_newer: Option<ResolvedExcludeNewer>,

    /// Build variants
    pub variant_configuration: Option<BTreeMap<String, Vec<VariantValue>>>,

    /// Build variant file contents
    pub variant_files: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone)]
pub struct InstantiateToolEnvironmentResult {
    /// The prefix of the tool environment.
    pub prefix: Prefix,

    /// The version of the requirement that was eventually installed.
    pub version: VersionWithSource,

    /// The version of the Pixi build API to use.
    pub api: PixiBuildApiVersion,
}

impl Hash for InstantiateToolEnvironmentSpec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let Self {
            requirement: (name, requirement),
            additional_requirements,
            constraints,
            build_environment,
            channels,
            exclude_newer,
            variant_configuration: variants,
            variant_files,
        } = self;
        name.hash(state);
        requirement.hash(state);
        additional_requirements
            .iter_specs()
            .sorted_by_key(|(name, _)| name.as_normalized())
            .for_each(|(name, spec)| {
                name.hash(state);
                spec.hash(state);
            });
        constraints
            .iter_specs()
            .sorted_by_key(|(name, _)| name.as_normalized())
            .for_each(|(name, spec)| {
                name.hash(state);
                spec.hash(state);
            });
        build_environment.hash(state);
        channels.hash(state);
        exclude_newer.hash(state);
        variants.hash(state);
        variant_files.hash(state);
    }
}

impl InstantiateToolEnvironmentSpec {
    /// Constructs a new default instance.
    pub fn new(
        package_name: rattler_conda_types::PackageName,
        requirement: PixiSpec,
        channels: Vec<ChannelUrl>,
    ) -> Self {
        Self {
            requirement: (package_name, requirement),
            additional_requirements: DependencyMap::default(),
            constraints: DependencyMap::default(),
            build_environment: BuildEnvironment::default(),
            channels,
            exclude_newer: None,
            variant_configuration: None,
            variant_files: None,
        }
    }

    pub fn cache_key(&self) -> String {
        let mut hasher = Xxh3::new();
        self.hash(&mut hasher);
        let unique_key = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());
        format!(
            "{}-{}",
            self.requirement.0.as_normalized(),
            BASE64_URL_SAFE_NO_PAD.encode(unique_key)
        )
    }
}

/// An error that may occur while trying to instantiate a tool environment.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum InstantiateToolEnvironmentError {
    #[error("failed to construct a tool prefix")]
    CreatePrefix(#[source] Arc<std::io::Error>),

    #[error("failed to acquire a lock for the tool prefix")]
    AcquireLock(#[source] Arc<std::io::Error>),

    #[error("failed to release lock for the tool prefix")]
    ReleaseLock(#[source] Arc<std::io::Error>),

    #[error("failed to update lock for the tool prefix")]
    UpdateLock(#[source] Arc<std::io::Error>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SolveEnvironment(Arc<SolvePixiEnvironmentError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    InstallEnvironment(Arc<InstallPixiEnvironmentError>),

    /// Error surfaced from the ephemeral-env Key. Holds the original
    /// [`crate::EphemeralEnvError`] so callers can inspect it.
    #[error(transparent)]
    #[diagnostic(transparent)]
    EphemeralEnv(Arc<crate::EphemeralEnvError>),

    #[error("The environment for the build backend package (`{} {}`) does not depend on `{}`. Without this package pixi has no way of knowing the API to use to communicate with the backend.", .build_backend.0.as_normalized(), .build_backend.1.to_string(), PIXI_BUILD_API_VERSION_NAME.as_normalized()
    )]
    #[diagnostic(help(
        "Modify the requirements on `{}` or contact the maintainers to ensure a dependency on `{}` is added.", .build_backend.0.as_normalized(), PIXI_BUILD_API_VERSION_NAME.as_normalized()
    ))]
    NoMatchingBackends {
        build_backend: Box<(PackageName, PixiSpec)>,
    },
}
