use pixi_build_types::procedures::conda_build_v1::{CondaBuildV1Params, CondaBuildV1Result};
use pixi_build_types::procedures::conda_outputs::{CondaOutputsParams, CondaOutputsResult};
use pixi_build_types::procedures::{
    initialize::{InitializeParams, InitializeResult},
    negotiate_capabilities::{NegotiateCapabilitiesParams, NegotiateCapabilitiesResult},
};

/// A trait that is used to instantiate a new protocol connection
/// and endpoint that can handle the RPC calls.
#[async_trait::async_trait]
pub trait ProtocolInstantiator: Send + Sync + 'static {
    /// Called when negotiating capabilities with the client.
    /// This is determine how the rest of the initialization will proceed.
    async fn negotiate_capabilities(
        params: NegotiateCapabilitiesParams,
    ) -> miette::Result<NegotiateCapabilitiesResult>;

    /// Called when the client requests initialization.
    /// Returns the protocol endpoint and the result of the initialization.
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> miette::Result<(Box<dyn Protocol + Send + Sync + 'static>, InitializeResult)>;
}

/// A trait that defines the protocol for a pixi build backend.
/// These are implemented by the different backends. Which
/// server as an endpoint for the RPC calls.
#[async_trait::async_trait]
pub trait Protocol {
    /// Called when the client requests outputs for a Conda package.
    async fn conda_outputs(
        &self,
        _params: CondaOutputsParams,
    ) -> miette::Result<CondaOutputsResult> {
        unimplemented!("conda_outputs not implemented");
    }

    /// Called when the client calls `conda/build_v1`.
    async fn conda_build_v1(
        &self,
        _params: CondaBuildV1Params,
    ) -> miette::Result<CondaBuildV1Result> {
        unimplemented!("conda_build_v1 not implemented");
    }
}
