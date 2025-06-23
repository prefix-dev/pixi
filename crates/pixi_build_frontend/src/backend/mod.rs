use pixi_build_types::{
    PixiBuildApiVersion,
    procedures::{
        conda_build::{CondaBuildParams, CondaBuildResult},
        conda_metadata::{CondaMetadataParams, CondaMetadataResult},
        conda_outputs::{CondaOutputsParams, CondaOutputsResult},
    },
};

mod stderr;

use crate::json_rpc::CommunicationError;

pub mod json_rpc;

pub struct Backend {
    /// The backend that is used to communicate with the build server.
    inner: BackendImplementation,

    /// The API version that the backend supports.
    pub api_version: PixiBuildApiVersion,
}

pub enum BackendImplementation {
    /// The backend is a JSON-RPC backend.
    JsonRpc(json_rpc::JsonRpcBackend),
}

impl From<json_rpc::JsonRpcBackend> for BackendImplementation {
    fn from(json_rpc: json_rpc::JsonRpcBackend) -> Self {
        BackendImplementation::JsonRpc(json_rpc)
    }
}

impl Backend {
    pub fn new(inner: BackendImplementation, api_version: PixiBuildApiVersion) -> Self {
        Self { inner, api_version }
    }

    pub fn identifier(&self) -> String {
        match &self.inner {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.identifier().to_string(),
        }
    }

    pub async fn conda_get_metadata(
        &self,
        params: CondaMetadataParams,
    ) -> Result<CondaMetadataResult, CommunicationError> {
        match &self.inner {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.conda_get_metadata(params).await,
        }
    }

    pub async fn conda_build<W: BackendOutputStream + Send + 'static>(
        &self,
        params: CondaBuildParams,
        output_stream: W,
    ) -> Result<CondaBuildResult, CommunicationError> {
        match &self.inner {
            BackendImplementation::JsonRpc(json_rpc) => {
                json_rpc.conda_build(params, output_stream).await
            }
        }
    }
}

pub trait BackendOutputStream {
    fn on_line(&mut self, line: String);
}

impl BackendOutputStream for () {
    fn on_line(&mut self, _line: String) {
        // No-op implementation
    }
}

impl<F: FnMut(String)> BackendOutputStream for F {
    fn on_line(&mut self, line: String) {
        self(line);
    }
}
