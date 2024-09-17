use pixi_build_types::procedures::{
    conda_build::{CondaBuildParams, CondaBuildResult},
    conda_metadata::{CondaMetadataParams, CondaMetadataResult},
};

use crate::{conda_build_protocol, pixi_protocol};

/// Top-level error type for protocol errors.
#[derive(Debug, thiserror::Error)]
pub enum FinishError {
    #[error(transparent)]
    Pixi(#[from] pixi_protocol::InitializeError),
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("source directory must be a directory")]
    NotADirectory,
    #[error("cannot find source directory '{0}'")]
    NotFound(String),
    #[error("loading manifest error '{0}'")]
    ManifestError(String),
    #[error("unable to discover communication protocol, currently expects pixi.toml or meta.yaml")]
    UnsupportedFormat,
}

/// A protocol describes how to communicate with a build backend. A build
/// backend is a tool that is invoked to generate certain output.
///
/// The frontend can support multiple backends, and the protocol is used to
/// determine which backend to use for a given source directory and how to
/// communicate with it.
///
///
/// The [`Protocol`] protocol is a generic implementation that uses a
/// client-server JSON-RPC interface to communicate with another tool.
///
/// Using this JSON-RPC interface means we can evolve the backend and frontend
/// tools as long as both tools can establish a shared protocol. The JSON-RPC
/// protocol is engineered in such a way that this is possible. This allows a
/// much newer frontend to still be able to interact with a very old backend
/// which is important if you want to be able to use very old packages in the
/// far future.
///
/// The conda-build and rattler-build implementations are a hard-coded
/// implementation and do not use a client-server model. Although technically
/// they could also be implemented using the client-server model it is more
/// ergonomic to add their implementation directly into the frontend because no
/// bridge executable is needed. We can always add this later too using the
/// existing protocol.
// I think because we mostly have a single variant in use, boxing does not make
// sense here.
#[allow(clippy::large_enum_variant)]
pub enum Protocol {
    Pixi(pixi_protocol::Protocol),
    CondaBuild(conda_build_protocol::Protocol),
}

impl From<pixi_protocol::Protocol> for Protocol {
    fn from(value: pixi_protocol::Protocol) -> Self {
        Self::Pixi(value)
    }
}

impl From<conda_build_protocol::Protocol> for Protocol {
    fn from(value: conda_build_protocol::Protocol) -> Self {
        Self::CondaBuild(value)
    }
}

impl Protocol {
    pub async fn get_conda_metadata(
        &self,
        request: &CondaMetadataParams,
    ) -> miette::Result<CondaMetadataResult> {
        match self {
            Self::Pixi(protocol) => protocol.get_conda_metadata(request).await,
            Self::CondaBuild(protocol) => protocol.get_conda_metadata(request),
        }
    }

    pub async fn conda_build(
        &self,
        request: &CondaBuildParams,
    ) -> miette::Result<CondaBuildResult> {
        match self {
            Self::Pixi(protocol) => protocol.conda_build(request).await,
            Self::CondaBuild(_) => unreachable!(),
        }
    }
}
