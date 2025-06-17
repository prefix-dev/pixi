use pixi_build_types::procedures::{
    conda_build::{CondaBuildParams, CondaBuildResult},
    conda_metadata::{CondaMetadataParams, CondaMetadataResult},
};

use crate::json_rpc::CommunicationError;

pub mod json_rpc;

pub enum Backend {
    /// The backend is a JSON-RPC backend.
    JsonRpc(json_rpc::JsonRpcBackend),
}

impl Backend {
    pub fn identifier(&self) -> String {
        match self {
            Backend::JsonRpc(json_rpc) => json_rpc.identifier().to_string(),
        }
    }

    pub async fn conda_get_metadata(
        &self,
        params: CondaMetadataParams,
    ) -> Result<CondaMetadataResult, CommunicationError> {
        match self {
            Backend::JsonRpc(json_rpc) => json_rpc.conda_get_metadata(params).await,
        }
    }

    pub async fn conda_build<W: BackendOutputStream + Send + 'static>(
        &self,
        params: CondaBuildParams,
        output_stream: W,
    ) -> Result<CondaBuildResult, CommunicationError> {
        match self {
            Backend::JsonRpc(json_rpc) => json_rpc.conda_build(params, output_stream).await,
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
