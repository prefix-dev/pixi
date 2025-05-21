use crate::install_pixi::{InstallPixiEnvironmentError, InstallPixiEnvironmentSpec};
use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    PixiEnvironmentSpec, SolvePixiEnvironmentError,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::prelude::BASE64_URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use futures::TryFutureExt;
use itertools::Itertools;
use miette::Diagnostic;
use pixi_build_frontend::EnabledProtocols;
use pixi_spec::PixiSpec;
use pixi_spec_containers::DependencyMap;
use pixi_utils::AsyncPrefixGuard;
use rattler_conda_types::{ChannelConfig, ChannelUrl, NamelessMatchSpec, prefix::Prefix};
use rattler_solve::{ChannelPriority, SolveStrategy};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

/// Specification for a tool environment. Tool environments are cached between
/// runs.
#[derive(Debug)]
pub struct InstantiateToolEnvironmentSpec {
    /// The main requirement of the tool environment.
    pub requirement: (rattler_conda_types::PackageName, PixiSpec),

    /// The requirements of the tool environment.
    pub additional_requirements: DependencyMap<rattler_conda_types::PackageName, PixiSpec>,

    /// Additional constraints applied to the environment.
    pub constraints: DependencyMap<rattler_conda_types::PackageName, NamelessMatchSpec>,

    /// The platform to instantiate the tool environment for.
    pub build_environment: BuildEnvironment,

    /// The channels to use for solving
    pub channels: Vec<ChannelUrl>,

    /// Exclude any packages after the first cut-off date.
    pub exclude_newer: Option<DateTime<Utc>>,

    /// The channel configuration to use for this environment.
    pub channel_config: ChannelConfig,

    /// The protocols that are enabled for source packages
    pub enabled_protocols: EnabledProtocols,
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
    }
}

impl InstantiateToolEnvironmentSpec {
    /// Constructs a new default instance.
    pub fn new(package_name: rattler_conda_types::PackageName, requirement: PixiSpec) -> Self {
        Self {
            requirement: (package_name, requirement),
            additional_requirements: DependencyMap::default(),
            constraints: DependencyMap::default(),
            build_environment: BuildEnvironment::default(),
            channels: vec![],
            exclude_newer: None,
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from(".")),
            enabled_protocols: EnabledProtocols::default(),
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
    ) -> Result<Prefix, CommandDispatcherError<InstantiateToolEnvironmentError>> {
        // Determine the cache key for the environment.
        let cache_key = self.cache_key();

        // Construct the prefix for the tool environment.
        let prefix = Prefix::create(command_queue.cache_dirs().build_backends().join(cache_key))
            .map_err(InstantiateToolEnvironmentError::CreatePrefix)?;

        // Acquire a lock on the tool prefix.
        let mut prefix_guard = AsyncPrefixGuard::new(prefix.path())
            .and_then(|guard| guard.write())
            .await
            .map_err(InstantiateToolEnvironmentError::AcquireLock)?;

        // If the environment already exists, we can return early.
        if prefix_guard.is_ready() {
            prefix_guard
                .finish()
                .await
                .map_err(InstantiateToolEnvironmentError::ReleaseLock)?;
            return Ok(prefix);
        }

        // Update the prefix to indicate that we are install it.
        prefix_guard
            .begin()
            .await
            .map_err(InstantiateToolEnvironmentError::UpdateLock)?;

        // Start by solving the environment.
        let target_platform = self.build_environment.host_platform;
        let solved_environment = command_queue
            .solve_pixi_environment(PixiEnvironmentSpec {
                dependencies: self
                    .additional_requirements
                    .into_specs()
                    .chain([self.requirement])
                    .collect(),
                constraints: self.constraints,
                build_environment: self.build_environment,
                exclude_newer: self.exclude_newer,
                channel_config: self.channel_config,
                channels: self.channels,
                enabled_protocols: self.enabled_protocols,
                installed: Vec::new(), // Install from scratch
                channel_priority: ChannelPriority::default(),
                strategy: SolveStrategy::default(),
            })
            .await
            .map_err_with(Box::new)
            .map_err_with(InstantiateToolEnvironmentError::SolveEnvironment)?;

        // Install the environment
        command_queue
            .install_pixi_environment(InstallPixiEnvironmentSpec {
                records: solved_environment,
                prefix: prefix.clone(),
                installed: None,
                platform: target_platform,
                force_reinstall: Default::default(),
            })
            .await
            .map_err_with(InstantiateToolEnvironmentError::InstallEnvironment)?;

        // Mark the environment as finished.
        prefix_guard
            .finish()
            .await
            .map_err(InstantiateToolEnvironmentError::UpdateLock)?;

        Ok(prefix)
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
    InstallEnvironment(InstallPixiEnvironmentError),
}
