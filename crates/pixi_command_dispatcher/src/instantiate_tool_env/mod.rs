use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::PathBuf,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD, prelude::BASE64_URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use futures::TryFutureExt;
use itertools::Itertools;
use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_build_types::{
    PIXI_BUILD_API_VERSION_NAME, PIXI_BUILD_API_VERSION_SPEC, PixiBuildApiVersion,
};
use pixi_spec::{BinarySpec, PixiSpec};
use pixi_spec_containers::DependencyMap;
use pixi_utils::AsyncPrefixGuard;
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, PackageName, VersionWithSource, prefix::Prefix,
};
use rattler_solve::{ChannelPriority, SolveStrategy};
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    PixiEnvironmentSpec, SolvePixiEnvironmentError,
    install_pixi::{InstallPixiEnvironmentError, InstallPixiEnvironmentSpec},
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

    /// Exclude any packages after the first cut-off date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_newer: Option<DateTime<Utc>>,

    /// The channel configuration to use for this environment.
    pub channel_config: ChannelConfig,

    /// Variants
    pub variants: Option<BTreeMap<String, Vec<String>>>,

    /// The protocols that are enabled for source packages
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
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
            channel_config,
            enabled_protocols,
            variants,
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
        channel_config.hash(state);
        enabled_protocols.hash(state);
        variants.hash(state);
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
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from(".")),
            enabled_protocols: EnabledProtocols::default(),
            variants: None,
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

    /// Instantiates a tool environment using the given command dispatcher.
    ///
    /// This method creates or reuses a cached environment based on the
    /// specification. The process includes:
    ///
    /// 1. Generating a unique cache key for the environment based on its
    ///    requirements
    /// 2. Checking if a matching environment already exists in the cache
    /// 3. If found, reusing the existing environment
    /// 4. If not found, solving and installing a new environment using
    ///    `CommandDispatcher::solve_pixi_environment`
    ///
    /// The method applies proper locking to ensure thread safety and handles
    /// concurrent access to the same environment appropriately. This prevents
    /// race conditions when multiple processes attempt to create the same
    /// tool environment simultaneously.
    pub async fn instantiate(
        self,
        command_queue: CommandDispatcher,
    ) -> Result<
        InstantiateToolEnvironmentResult,
        CommandDispatcherError<InstantiateToolEnvironmentError>,
    > {
        tracing::debug!(
            "Installing tool env for: {}",
            &self.requirement.0.as_source()
        );

        // Determine the cache key for the environment.
        let cache_key = self.cache_key();

        // Construct a spec that will ensure that the backend is compatible with the
        // Pixi build API version that we support.
        let constraints = {
            let mut constraints = self.constraints;
            constraints.insert(
                PIXI_BUILD_API_VERSION_NAME.clone(),
                BinarySpec::Version(PIXI_BUILD_API_VERSION_SPEC.clone()),
            );
            constraints
        };

        // Start by solving the environment.
        let name = self.requirement.0.as_source().to_string();
        let solved_environment = command_queue
            .solve_pixi_environment(PixiEnvironmentSpec {
                name: Some(name.clone()),
                dependencies: self
                    .additional_requirements
                    .into_specs()
                    .chain([self.requirement.clone()])
                    .collect(),
                constraints,
                build_environment: self.build_environment.clone(),
                exclude_newer: self.exclude_newer,
                channel_config: self.channel_config.clone(),
                channels: self.channels.clone(),
                enabled_protocols: self.enabled_protocols.clone(),
                installed: Vec::new(), // Install from scratch
                channel_priority: ChannelPriority::default(),
                variants: self.variants.clone(),
                strategy: SolveStrategy::default(),
            })
            .await
            .map_err_with(Box::new)
            .map_err_with(InstantiateToolEnvironmentError::SolveEnvironment)?;

        // Ensure that the solution contains matching api version package
        let Some(api_version) = solved_environment
            .iter()
            .find(|r| r.package_record().name == *PIXI_BUILD_API_VERSION_NAME)
            .map(|r| r.package_record().version.as_ref())
            .and_then(PixiBuildApiVersion::from_version)
        else {
            return Err(CommandDispatcherError::Failed(
                InstantiateToolEnvironmentError::NoMatchingBackends {
                    build_backend: self.requirement,
                },
            ));
        };

        // Extract the version of the main requirement package.
        let version = solved_environment
            .iter()
            .find(|r| r.package_record().name == self.requirement.0)
            .expect("The solved environment should always contain the main requirement package")
            .package_record()
            .version
            .clone();

        // Construct the prefix for the tool environment.
        let prefix = Prefix::create(command_queue.cache_dirs().build_backends().join(cache_key))
            .map_err(InstantiateToolEnvironmentError::CreatePrefix)
            .map_err(CommandDispatcherError::Failed)?;

        // Acquire a lock on the tool prefix.
        let mut prefix_guard = AsyncPrefixGuard::new(prefix.path())
            .and_then(|guard| guard.write())
            .await
            .map_err(InstantiateToolEnvironmentError::AcquireLock)
            .map_err(CommandDispatcherError::Failed)?;

        // Update the prefix to indicate that we are install it.
        prefix_guard
            .begin()
            .await
            .map_err(InstantiateToolEnvironmentError::UpdateLock)
            .map_err(CommandDispatcherError::Failed)?;

        // Install the environment
        command_queue
            .install_pixi_environment(InstallPixiEnvironmentSpec {
                name,
                records: solved_environment,
                prefix: prefix.clone(),
                installed: None,
                build_environment: self.build_environment,
                ignore_packages: None,
                force_reinstall: Default::default(),
                channels: self.channels,
                channel_config: self.channel_config,
                variants: self.variants,
                enabled_protocols: self.enabled_protocols,
            })
            .await
            .map_err_with(Box::new)
            .map_err_with(InstantiateToolEnvironmentError::InstallEnvironment)?;

        // Mark the environment as finished.
        prefix_guard
            .finish()
            .await
            .map_err(InstantiateToolEnvironmentError::UpdateLock)
            .map_err(CommandDispatcherError::Failed)?;

        Ok(InstantiateToolEnvironmentResult {
            prefix,
            version,
            api: api_version,
        })
    }
}

/// An error that may occur while trying to instantiate a tool environment.
#[derive(Debug, Error, Diagnostic)]
pub enum InstantiateToolEnvironmentError {
    #[error("failed to construct a tool prefix")]
    CreatePrefix(#[source] std::io::Error),

    #[error("failed to acquire a lock for the tool prefix")]
    AcquireLock(#[source] std::io::Error),

    #[error("failed to release lock for the tool prefix")]
    ReleaseLock(#[source] std::io::Error),

    #[error("failed to update lock for the tool prefix")]
    UpdateLock(#[source] std::io::Error),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SolveEnvironment(Box<SolvePixiEnvironmentError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    InstallEnvironment(Box<InstallPixiEnvironmentError>),

    #[error("The environment for the build backend package (`{} {}`) does not depend on `{}`. Without this package pixi has no way of knowing the API to use to communicate with the backend.", .build_backend.0.as_normalized(), .build_backend.1.to_string(), PIXI_BUILD_API_VERSION_NAME.as_normalized()
    )]
    #[diagnostic(help(
        "Modify the requirements on `{}` or contact the maintainers to ensure a dependency on `{}` is added.", .build_backend.0.as_normalized(), PIXI_BUILD_API_VERSION_NAME.as_normalized()
    ))]
    NoMatchingBackends {
        build_backend: (PackageName, PixiSpec),
    },
}
