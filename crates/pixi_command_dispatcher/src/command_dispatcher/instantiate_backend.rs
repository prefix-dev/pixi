use miette::Diagnostic;
use pixi_build_discovery::{
    BackendInitializationParams, BackendSpec, CommandSpec, EnabledProtocols,
};
use pixi_build_frontend::{
    Backend, BackendOverride, json_rpc,
    json_rpc::{CommunicationError, JsonRpcBackend},
    tool::{IsolatedTool, SystemTool, Tool},
};
use pixi_build_types::{PixiBuildApiVersion, procedures::initialize::InitializeParams};
use pixi_spec::{SourceLocationSpec, SpecConversionError};
use rattler_conda_types::ChannelConfig;
use rattler_shell::{
    activation::{ActivationError, ActivationVariables, Activator},
    shell::ShellEnum,
};
use rattler_virtual_packages::DetectVirtualPackageError;
use thiserror::Error;

use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherErrorResultExt,
    command_dispatcher::error::CommandDispatcherError,
    instantiate_tool_env::{
        InstantiateToolEnvironmentError, InstantiateToolEnvironmentResult,
        InstantiateToolEnvironmentSpec,
    },
};

#[derive(Debug)]
pub struct InstantiateBackendSpec {
    /// The backend specification
    pub backend_spec: BackendSpec,

    /// The parameters to initialize the backend with
    pub init_params: BackendInitializationParams,

    /// The channel configuration to use for any source packages required by the
    /// backend.
    pub channel_config: ChannelConfig,

    /// The protocols that are enabled for discovering source packages
    pub enabled_protocols: EnabledProtocols,
}

impl CommandDispatcher {
    /// Instantiate a build backend
    pub async fn instantiate_backend(
        &self,
        spec: InstantiateBackendSpec,
    ) -> Result<Backend, CommandDispatcherError<InstantiateBackendError>> {
        let BackendSpec::JsonRpc(backend_spec) = spec.backend_spec;

        let source_dir = if let Some(SourceLocationSpec::Path(path)) = spec.init_params.source {
            path.resolve(&spec.init_params.source_anchor)
                .map_err(InstantiateBackendError::from)
                .map_err(CommandDispatcherError::Failed)?
        } else {
            spec.init_params.source_anchor
        };

        let command_spec = match self.build_backend_overrides() {
            BackendOverride::System(overridden_backends) => overridden_backends
                .named_backend_override(&backend_spec.name)
                .unwrap_or(backend_spec.command),
            BackendOverride::InMemory(memory) => {
                let backend = memory.backend_override(&backend_spec.name);

                if let Some(in_mem) = backend {
                    let memory = in_mem
                        .initialize(InitializeParams {
                            manifest_path: spec.init_params.manifest_path,
                            source_dir: Some(source_dir),
                            workspace_root: Some(spec.init_params.workspace_root),
                            cache_directory: Some(self.cache_dirs().root().clone()),
                            project_model: spec.init_params.project_model.map(Into::into),
                            configuration: spec.init_params.configuration,
                            target_configuration: spec.init_params.target_configuration,
                        })
                        .map_err(InstantiateBackendError::InMemoryError)
                        .map_err(CommandDispatcherError::Failed)?;
                    return Ok(Backend::new(memory.into(), in_mem.api_version()));
                } else {
                    backend_spec.command
                }
            }
        };

        let (tool, api_version) = match command_spec {
            CommandSpec::System(system_spec) => (
                Tool::System(SystemTool::new(
                    system_spec.command.unwrap_or(backend_spec.name),
                )),
                // Assume the latest version of the backend
                PixiBuildApiVersion::current(),
            ),
            CommandSpec::EnvironmentSpec(env_spec) => {
                let (tool_platform, tool_platform_virtual_packages) = self.tool_platform();
                let InstantiateToolEnvironmentResult {
                    prefix,
                    version,
                    api,
                } = self
                    .instantiate_tool_environment(InstantiateToolEnvironmentSpec {
                        requirement: env_spec.requirement,
                        additional_requirements: env_spec.additional_requirements,
                        constraints: env_spec.constraints,
                        build_environment: BuildEnvironment {
                            host_platform: tool_platform,
                            build_platform: tool_platform,
                            host_virtual_packages: tool_platform_virtual_packages.to_vec(),
                            build_virtual_packages: tool_platform_virtual_packages.to_vec(),
                        },
                        channels: env_spec.channels,
                        exclude_newer: None,
                        variants: None,
                        channel_config: spec.channel_config,
                        enabled_protocols: spec.enabled_protocols,
                    })
                    .await
                    .map_err_with(InstantiateBackendError::from)?;

                // Get the activation scripts
                let activator =
                    Activator::from_path(prefix.path(), ShellEnum::default(), tool_platform)
                        .map_err(InstantiateBackendError::from)
                        .map_err(CommandDispatcherError::Failed)?;

                let activation_scripts = activator
                    .run_activation(ActivationVariables::from_env().unwrap_or_default(), None)
                    .map_err(InstantiateBackendError::from)
                    .map_err(CommandDispatcherError::Failed)?;

                (
                    Tool::from(IsolatedTool::new(
                        env_spec.command.unwrap_or(backend_spec.name),
                        Some(version),
                        prefix.path().to_path_buf(),
                        activation_scripts,
                    )),
                    api,
                )
            }
        };

        // Add debug information about what the backend supports.
        tracing::info!(
            "Instantiated backend {}{}, negotiated API version {}",
            tool.executable(),
            tool.version()
                .map_or_else(String::new, |v| format!("@{}", v)),
            api_version,
        );

        // Make sure that the project model is compatible with the API version.
        if !api_version.supports_name_none()
            && spec
                .init_params
                .project_model
                .as_ref()
                .is_some_and(|p| p.name.is_none())
        {
            return Err(CommandDispatcherError::Failed(
                InstantiateBackendError::SpecConversionError(SpecConversionError::MissingName),
            ));
        }

        JsonRpcBackend::setup(
            source_dir,
            spec.init_params.manifest_path,
            spec.init_params.workspace_root,
            spec.init_params.project_model,
            spec.init_params.configuration,
            spec.init_params.target_configuration,
            Some(self.cache_dirs().root().clone()),
            tool,
        )
        .await
        .map_err(InstantiateBackendError::from)
        .map_err(CommandDispatcherError::Failed)
        .map(|backend| Backend::new(backend.into(), api_version))
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum InstantiateBackendError {
    /// The command dispatcher could not be initialized.
    #[error(transparent)]
    #[diagnostic(transparent)]
    JsonRpc(#[from] json_rpc::InitializeError),

    /// The command dispatcher could not be initialized.
    #[error(transparent)]
    #[diagnostic(transparent)]
    InMemoryError(CommunicationError),

    /// Could not detect the virtual packages for the system
    #[error(transparent)]
    VirtualPackages(#[from] DetectVirtualPackageError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    InstantiateToolEnvironment(#[from] InstantiateToolEnvironmentError),

    #[error("failed to run activation for the backend tool")]
    Activation(#[from] ActivationError),

    #[error(transparent)]
    SpecConversionError(#[from] SpecConversionError),
}
