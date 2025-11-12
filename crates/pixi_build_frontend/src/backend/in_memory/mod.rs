//! This module defines the in-memory build backend interface and some
//! implementations. Together with [`crate::BackendOverride`], it allows you to
//! create a build backend that runs completely in memory, without the need to
//! install any external tools or processes.
//!
//! This is especially useful for testing purposes.

use pixi_build_types::{
    BackendCapabilities, PixiBuildApiVersion,
    procedures::{
        conda_build_v1::{CondaBuildV1Params, CondaBuildV1Result},
        conda_outputs::{CondaOutputsParams, CondaOutputsResult},
        initialize::InitializeParams,
    },
};
use std::{fmt::Debug, sync::Arc};

use crate::{BackendOutputStream, json_rpc::CommunicationError};

/// A factory trait that allows instantiating a specific in-memory build
/// backend.
pub trait InMemoryBackendInstantiator {
    type Backend: InMemoryBackend;

    fn initialize(
        &self,
        params: InitializeParams,
    ) -> Result<Self::Backend, Box<CommunicationError>>;

    fn identifier(&self) -> &str;

    /// Returns the api version that this backend supports.
    fn api_version(&self) -> PixiBuildApiVersion {
        PixiBuildApiVersion::current()
    }
}

#[allow(unused_variables)]
pub trait InMemoryBackend: Send {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::default()
    }

    fn identifier(&self) -> &str;

    fn conda_build_v1(
        &self,
        params: CondaBuildV1Params,
        output_stream: &(dyn BackendOutputStream + Send + 'static),
    ) -> Result<CondaBuildV1Result, Box<CommunicationError>> {
        unimplemented!()
    }

    /// Returns the outputs that this backend can produce.
    fn conda_outputs(
        &self,
        params: CondaOutputsParams,
        output_stream: &(dyn BackendOutputStream + Send + 'static),
    ) -> Result<CondaOutputsResult, Box<CommunicationError>> {
        unimplemented!()
    }
}

type ErasedInMemoryBackend = Box<dyn InMemoryBackend + 'static>;
type ErasedInitializationFn = dyn Fn(InitializeParams) -> Result<ErasedInMemoryBackend, Box<CommunicationError>>
    + Send
    + Sync;

/// A helper type that erases the type of the in-memory build backend.
#[derive(Clone)]
pub struct BoxedInMemoryBackend {
    identifier: String,
    initialize: Arc<ErasedInitializationFn>,
    api_version: PixiBuildApiVersion,
}

impl BoxedInMemoryBackend {
    /// Initializes the backend with the given parameters.
    pub fn initialize(
        &self,
        params: InitializeParams,
    ) -> Result<Box<dyn InMemoryBackend>, Box<CommunicationError>> {
        (self.initialize)(params)
    }

    /// Returns the api version that this backend supports.
    pub fn api_version(&self) -> PixiBuildApiVersion {
        self.api_version
    }
}

impl Debug for BoxedInMemoryBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoxedInMemoryBackend")
            .field("identifier", &self.identifier)
            .finish()
    }
}

impl<T: InMemoryBackendInstantiator + Send + Sync + 'static> From<T> for BoxedInMemoryBackend {
    fn from(instantiator: T) -> Self {
        Self {
            identifier: instantiator.identifier().to_owned(),
            api_version: instantiator.api_version(),
            initialize: Arc::new(move |params| {
                instantiator
                    .initialize(params)
                    .map(|b| Box::new(b) as Box<dyn InMemoryBackend>)
            }),
        }
    }
}
