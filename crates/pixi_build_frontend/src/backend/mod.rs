use pixi_build_types::{
    BackendCapabilities, PixiBuildApiVersion,
    procedures::{
        conda_build::{CondaBuildParams, CondaBuildResult},
        conda_build_v2::{CondaBuildV2Params, CondaBuildV2Result},
        conda_metadata::{CondaMetadataParams, CondaMetadataResult},
        conda_outputs::{CondaOutputsParams, CondaOutputsResult},
    },
};

mod stderr;

use crate::json_rpc::CommunicationError;

pub mod json_rpc;

#[derive(Debug)]
pub struct Backend {
    /// The backend that is used to communicate with the build server.
    inner: BackendImplementation,

    /// The API version that the backend supports.
    api_version: PixiBuildApiVersion,

    /// The backend capabilities that the backend support also taking into
    /// account the API version.
    capabilities: BackendCapabilities,
}

#[derive(Debug)]
pub enum BackendImplementation {
    /// The backend is a JSON-RPC backend.
    JsonRpc(json_rpc::JsonRpcBackend),
}

impl BackendImplementation {
    pub fn capabilities(&self) -> &BackendCapabilities {
        match self {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.capabilities(),
        }
    }

    pub fn identifier(&self) -> &str {
        match self {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.identifier(),
        }
    }
}

impl From<json_rpc::JsonRpcBackend> for BackendImplementation {
    fn from(json_rpc: json_rpc::JsonRpcBackend) -> Self {
        BackendImplementation::JsonRpc(json_rpc)
    }
}

impl Backend {
    pub fn new(inner: BackendImplementation, api_version: PixiBuildApiVersion) -> Self {
        let capabilities = inner.capabilities().mask_with_api_version(&api_version);
        Self {
            inner,
            api_version,
            capabilities,
        }
    }

    /// Returns an identifier for the backend. This is useful for debugging
    /// purposes mostly.
    pub fn identifier(&self) -> &str {
        self.inner.identifier()
    }

    /// Returns the capabilities of the backend. This takes into account both
    /// the actual capabilities of the backend and the API version that is in
    /// use.
    ///
    /// Sometimes backends provide more capabilities that the API version that
    /// we established. This can happen when the backend already implemented
    /// some capabilities both not all for a particular API version.
    pub fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    /// Returns the API version that was used to establish the backend.
    pub fn api_version(&self) -> PixiBuildApiVersion {
        self.api_version
    }

    pub async fn conda_get_metadata(
        &self,
        params: CondaMetadataParams,
    ) -> Result<CondaMetadataResult, CommunicationError> {
        assert!(
            self.inner.capabilities().provides_conda_metadata(),
            "This backend does not support the conda get metadata procedure"
        );
        match &self.inner {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.conda_get_metadata(params).await,
        }
    }

    pub async fn conda_build<W: BackendOutputStream + Send + 'static>(
        &self,
        params: CondaBuildParams,
        output_stream: W,
    ) -> Result<CondaBuildResult, CommunicationError> {
        assert!(
            self.inner.capabilities().provides_conda_build(),
            "This backend does not support the conda build procedure"
        );
        match &self.inner {
            BackendImplementation::JsonRpc(json_rpc) => {
                json_rpc.conda_build(params, output_stream).await
            }
        }
    }

    pub async fn conda_build_v2<W: BackendOutputStream + Send + 'static>(
        &self,
        params: CondaBuildV2Params,
        output_stream: W,
    ) -> Result<CondaBuildV2Result, CommunicationError> {
        assert!(
            self.inner.capabilities().provides_conda_build_v2(),
            "This backend does not support the conda build v2 procedure"
        );
        match &self.inner {
            BackendImplementation::JsonRpc(json_rpc) => {
                json_rpc.conda_build_v2(params, output_stream).await
            }
        }
    }

    /// Returns the outputs that this backend can produce.
    pub async fn conda_outputs(
        &self,
        params: CondaOutputsParams,
    ) -> Result<CondaOutputsResult, CommunicationError> {
        assert!(
            self.inner.capabilities().provides_conda_outputs(),
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
