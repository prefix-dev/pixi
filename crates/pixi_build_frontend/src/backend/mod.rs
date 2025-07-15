use std::fmt::{Debug, Formatter};

use crate::in_memory;
use crate::in_memory::InMemoryBackend;
use pixi_build_types::{
    BackendCapabilities, PixiBuildApiVersion,
    procedures::{
        conda_build_v0::{CondaBuildParams, CondaBuildResult},
        conda_build_v1::{CondaBuildV1Params, CondaBuildV1Result},
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

pub enum BackendImplementation {
    /// The backend is a JSON-RPC backend.
    JsonRpc(json_rpc::JsonRpcBackend),

    /// An in memory backend.
    InMemory(Box<dyn InMemoryBackend>),
}

impl Debug for BackendImplementation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.fmt(f),
            BackendImplementation::InMemory(backend) => f
                .debug_struct("InMemoryBackend")
                .field("identifier", &backend.identifier())
                .finish(),
        }
    }
}

impl BackendImplementation {
    pub fn capabilities(&self) -> BackendCapabilities {
        match self {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.capabilities().clone(),
            BackendImplementation::InMemory(in_memory) => in_memory.capabilities(),
        }
    }

    pub fn identifier(&self) -> &str {
        match self {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.identifier(),
            BackendImplementation::InMemory(in_memory) => in_memory.identifier(),
        }
    }
}

impl From<json_rpc::JsonRpcBackend> for BackendImplementation {
    fn from(json_rpc: json_rpc::JsonRpcBackend) -> Self {
        BackendImplementation::JsonRpc(json_rpc)
    }
}

impl From<Box<dyn in_memory::InMemoryBackend>> for BackendImplementation {
    fn from(in_memory: Box<dyn in_memory::InMemoryBackend>) -> Self {
        BackendImplementation::InMemory(in_memory)
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
            BackendImplementation::InMemory(in_memory) => in_memory.conda_get_metadata(params),
        }
    }

    pub async fn conda_build_v0<W: BackendOutputStream + Send + 'static>(
        &self,
        params: CondaBuildParams,
        output_stream: W,
    ) -> Result<CondaBuildResult, CommunicationError> {
        assert!(
            self.inner.capabilities().provides_conda_build_v0(),
            "This backend does not support the conda build v0 procedure"
        );
        match &self.inner {
            BackendImplementation::JsonRpc(json_rpc) => {
                json_rpc.conda_build_v0(params, output_stream).await
            }
            BackendImplementation::InMemory(in_memory) => {
                in_memory.conda_build_v0(params, &output_stream)
            }
        }
    }

    pub async fn conda_build_v1<W: BackendOutputStream + Send + 'static>(
        &self,
        params: CondaBuildV1Params,
        output_stream: W,
    ) -> Result<CondaBuildV1Result, CommunicationError> {
        assert!(
            self.inner.capabilities().provides_conda_build_v1(),
            "This backend does not support the conda build v1 procedure"
        );
        match &self.inner {
            BackendImplementation::JsonRpc(json_rpc) => {
                json_rpc.conda_build_v1(params, output_stream).await
            }
            BackendImplementation::InMemory(in_memory) => {
                in_memory.conda_build_v1(params, &output_stream)
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
            BackendImplementation::InMemory(in_memory) => in_memory.conda_outputs(params),
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
