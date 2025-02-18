use std::{path::PathBuf, sync::Arc};

use miette::{Diagnostic, IntoDiagnostic};
use pixi_build_types::procedures::{
    conda_build::{CondaBuildParams, CondaBuildResult},
    conda_metadata::{CondaMetadataParams, CondaMetadataResult},
};

use crate::{
    protocols::{
        builders::{conda_protocol, pixi_protocol, rattler_build_protocol},
        JsonRPCBuildProtocol,
    },
    CondaBuildReporter, CondaMetadataReporter,
};

/// Top-level error type for protocol errors.
#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum FinishError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Pixi(#[from] pixi_protocol::FinishError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    CondaBuild(#[from] conda_protocol::FinishError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    RattlerBuild(#[from] rattler_build_protocol::FinishError),
}

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum DiscoveryError {
    #[error(
        "failed to discover a valid project manifest, the source does not refer to a directory"
    )]
    NotADirectory,

    #[error("failed to discover a valid project manifest, the source path '{}' could not be found", .0.display())]
    NotFound(PathBuf),

    #[error("the source directory does not contain a supported manifest")]
    #[diagnostic(help(
        "Ensure that the source directory contains a valid pixi.toml or meta.yaml file."
    ))]
    UnsupportedFormat,

    #[error(transparent)]
    #[diagnostic(transparent)]
    Pixi(#[from] pixi_protocol::ProtocolBuildError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    RattlerBuild(#[from] rattler_build_protocol::ProtocolBuildError),
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
#[derive(Debug)]
pub enum Protocol {
    PixiBuild(JsonRPCBuildProtocol),
    // It should be more like subprocess protocol
    // as we invoke the tool directly
    CondaBuild(conda_protocol::Protocol),
}

impl From<JsonRPCBuildProtocol> for Protocol {
    fn from(value: JsonRPCBuildProtocol) -> Self {
        Self::PixiBuild(value)
    }
}

impl From<conda_protocol::Protocol> for Protocol {
    fn from(value: conda_protocol::Protocol) -> Self {
        Self::CondaBuild(value)
    }
}

impl Protocol {
    /// Returns the root manifest files of the source directory. These indicate
    /// the files that are used to determine the build configuration.
    pub fn manifests(&self) -> Vec<String> {
        match self {
            Self::PixiBuild(protocol) => protocol.manifests(),
            Self::CondaBuild(protocol) => protocol.manifests(),
        }
    }

    pub async fn conda_get_metadata(
        &self,
        request: &CondaMetadataParams,
        reporter: Arc<dyn CondaMetadataReporter>,
    ) -> miette::Result<CondaMetadataResult> {
        match self {
            Self::PixiBuild(protocol) => Ok(protocol
                .conda_get_metadata(request, reporter.as_ref())
                .await?),
            Self::CondaBuild(protocol) => protocol.conda_get_metadata(request),
        }
    }

    pub async fn conda_build(
        &self,
        request: &CondaBuildParams,
        reporter: Arc<dyn CondaBuildReporter>,
    ) -> miette::Result<CondaBuildResult> {
        match self {
            Self::PixiBuild(protocol) => protocol
                .conda_build(request, reporter.as_ref())
                .await
                .into_diagnostic(),
            Self::CondaBuild(_) => unreachable!(),
        }
    }

    pub fn identifier(&self) -> &str {
        match self {
            Self::PixiBuild(protocol) => protocol.backend_identifier(),
            Self::CondaBuild(protocol) => protocol.backend_identifier(),
        }
    }
}
