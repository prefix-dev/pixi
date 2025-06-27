use pixi_build_types::{
    BackendCapabilities, PixiBuildApiVersion,
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

    pub fn capabilities(&self) -> &BackendCapabilities {
        match &self.inner {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.capabilities(),
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

    /// Returns the outputs that this backend can produce.
    pub async fn conda_outputs(
        &self,
        params: CondaOutputsParams,
    ) -> Result<CondaOutputsResult, CommunicationError> {
        assert!(
            self.api_version.0 >= 1,
            "This backend does not support the conda outputs procedure"
        );
        match &self.inner {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.conda_outputs(params).await,
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
