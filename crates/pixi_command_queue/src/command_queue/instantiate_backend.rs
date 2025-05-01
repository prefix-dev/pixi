use pixi_build_frontend::{
    Backend, BackendInitializationParams, BackendSpec, CommandSpec, JsonRpcBackendSpec, json_rpc,
    json_rpc::{InitializeError, JsonRpcBackend},
    tool::{SystemTool, Tool},
};

use crate::{CommandQueue, CommandQueueError};

impl CommandQueue {
    /// Instantiate a build backend
    pub async fn instantiate_backend(
        &self,
        backend_spec: BackendSpec,
        init_params: BackendInitializationParams,
    ) -> Result<Backend, CommandQueueError<InitializeError>> {
        match backend_spec {
            BackendSpec::JsonRpc(spec) => {
                let backend = self.instantiate_json_rpc_backend(spec, init_params).await?;
                Ok(Backend::JsonRpc(backend))
            }
        }
    }

    /// Instantiate a JSON-RPC build backend.
    async fn instantiate_json_rpc_backend(
        &self,
        JsonRpcBackendSpec { name, command }: JsonRpcBackendSpec,
        init_params: BackendInitializationParams,
    ) -> Result<JsonRpcBackend, CommandQueueError<json_rpc::InitializeError>> {
        let command_spec = self
            .build_backend_overrides()
            .named_backend_override(&name)
            .unwrap_or(command);

        let tool = match command_spec {
            CommandSpec::EnvironmentSpec(_env_spec) => {
                unimplemented!("Environment spec is not implemented yet");
            }
            CommandSpec::System(system_spec) => {
                Tool::System(SystemTool::new(system_spec.command.unwrap_or(name)))
            }
        };

        JsonRpcBackend::setup(
            init_params.manifest_path,
            init_params.project_model,
            init_params.configuration,
            Some(self.cache_dirs().root().clone()),
            tool,
        )
        .await
        .map_err(CommandQueueError::Failed)
    }
}
