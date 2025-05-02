use crate::command_queue::error::CommandQueueError;
use crate::instantiate_tool_env::{
    InstantiateToolEnvironmentError, InstantiateToolEnvironmentSpec,
};
use crate::{BuildEnvironment, CommandQueue, CommandQueueErrorResultExt};
use miette::Diagnostic;
use pixi_build_frontend::tool::IsolatedTool;
use pixi_build_frontend::{
    Backend, BackendInitializationParams, BackendSpec, CommandSpec, EnabledProtocols, json_rpc,
    json_rpc::JsonRpcBackend,
    tool::{SystemTool, Tool},
};
use pixi_spec::PixiSpec;
use rattler_conda_types::ChannelConfig;
use rattler_shell::{
    activation::{ActivationError, ActivationVariables, Activator},
    shell::ShellEnum,
};
use rattler_virtual_packages::DetectVirtualPackageError;
use thiserror::Error;

#[derive(Debug)]
pub struct InstantiateBackendSpec {
    /// The backend specification
    pub backend_spec: BackendSpec,

    /// The parameters to initialize the backend with
    pub init_params: BackendInitializationParams,

    /// The channel configuration to use for any source packages required by the backend.
    pub channel_config: ChannelConfig,

    /// The platform to instantiate the backend for.
    pub build_environment: BuildEnvironment,

    /// The protocols that are enabled for discovering source packages
    pub enabled_protocols: EnabledProtocols,
}

impl CommandQueue {
    /// Instantiate a build backend
    pub async fn instantiate_backend(
        &self,
        spec: InstantiateBackendSpec,
    ) -> Result<Backend, CommandQueueError<InstantiateBackendError>> {
        let BackendSpec::JsonRpc(backend_spec) = spec.backend_spec;

        let command_spec = self
            .build_backend_overrides()
            .named_backend_override(&backend_spec.name)
            .unwrap_or(backend_spec.command);

        let tool = match command_spec {
            CommandSpec::System(system_spec) => Tool::System(SystemTool::new(
                system_spec.command.unwrap_or(backend_spec.name),
            )),
            CommandSpec::EnvironmentSpec(env_spec) => {
                let target_platform = spec.build_environment.host_platform;
                let prefix = self
                    .instantiate_tool_environment(InstantiateToolEnvironmentSpec {
                        requirement: (
                            env_spec.requirement.0,
                            PixiSpec::from_nameless_matchspec(
                                env_spec.requirement.1,
                                &spec.channel_config,
                            ),
                        ),
                        additional_requirements: env_spec
                            .additional_requirements
                            .into_specs()
                            .map(|(name, nameless)| {
                                (
                                    name,
                                    PixiSpec::from_nameless_matchspec(
                                        nameless,
                                        &spec.channel_config,
                                    ),
                                )
                            })
                            .collect(),
                        constraints: env_spec.constraints,
                        build_environment: spec.build_environment,
                        channels: env_spec.channels,
                        exclude_newer: None,
                        channel_config: spec.channel_config,
                        enabled_protocols: spec.enabled_protocols,
                    })
                    .await
                    .map_err_with(InstantiateBackendError::from)?;

                // Get the activation scripts
                let activator =
                    Activator::from_path(prefix.path(), ShellEnum::default(), target_platform)
                        .map_err(InstantiateBackendError::from)?;

                let activation_scripts = activator
                    .run_activation(ActivationVariables::from_env().unwrap_or_default(), None)
                    .map_err(InstantiateBackendError::from)?;

                Tool::from(IsolatedTool::new(
                    env_spec.command.unwrap_or(backend_spec.name),
                    prefix.path().to_path_buf(),
                    activation_scripts,
                ))
            }
        };

        JsonRpcBackend::setup(
            spec.init_params.manifest_path,
            spec.init_params.project_model,
            spec.init_params.configuration,
            Some(self.cache_dirs().root().clone()),
            tool,
        )
        .await
        .map_err(InstantiateBackendError::from)
        .map_err(CommandQueueError::Failed)
        .map(Backend::JsonRpc)
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum InstantiateBackendError {
    /// The command queue could not be initialized.
    #[error(transparent)]
    #[diagnostic(transparent)]
    JsonRpc(#[from] json_rpc::InitializeError),

    /// Could not detect the virtual packages for the system
    #[error(transparent)]
    VirtualPackages(#[from] DetectVirtualPackageError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    InstantiateToolEnvironment(#[from] InstantiateToolEnvironmentError),

    #[error("failed to run activation for the backend tool")]
    Activation(#[from] ActivationError),
}
