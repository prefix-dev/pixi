use rattler_conda_types::ChannelConfig;
use std::path::Path;

use crate::{
    conda_build, pixi,
    tool::{Tool, ToolSpec},
};

#[derive(Debug)]
pub(crate) enum ProtocolBuilder {
    /// A pixi project.
    Pixi(pixi::ProtocolBuilder),

    /// A directory containing a `meta.yaml` that can be interpreted by
    /// conda-build.
    CondaBuild(conda_build::ProtocolBuilder),
}

impl From<pixi::ProtocolBuilder> for ProtocolBuilder {
    fn from(value: pixi::ProtocolBuilder) -> Self {
        Self::Pixi(value)
    }
}

impl From<conda_build::ProtocolBuilder> for ProtocolBuilder {
    fn from(value: conda_build::ProtocolBuilder) -> Self {
        Self::CondaBuild(value)
    }
}

/// Top-level error type for protocol errors.
#[derive(Debug, thiserror::Error)]
pub enum FinishError {
    #[error("error while setting up pixi protocol")]
    Pixi(#[from] pixi::InitializeError),
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("source directory must be a directory")]
    NotADirectory,
    #[error("cannot find source directory '{0}'")]
    NotFound(String),
    #[error("loading manifest error '{0}'")]
    ManifestError(String),
    #[error("unsupported format, unable to discover protocol")]
    UnsupportedFormat,
}

impl ProtocolBuilder {
    /// Discovers the protocol for the given source directory.
    pub fn discover(source_dir: &Path) -> Result<Self, DiscoveryError> {
        if source_dir.is_file() {
            return Err(DiscoveryError::NotADirectory);
        } else if !source_dir.is_dir() {
            return Err(DiscoveryError::NotFound(source_dir.display().to_string()));
        }

        // TODO: get rid of the converted miette error
        // Try to discover as a pixi project
        if let Some(protocol) = pixi::ProtocolBuilder::discover(source_dir)
            .map_err(|e| DiscoveryError::ManifestError(e.to_string()))?
        {
            return Ok(protocol.into());
        }

        // Try to discover as a conda build project
        // Unwrap because error is Infallible
        if let Some(protocol) = conda_build::ProtocolBuilder::discover(source_dir).unwrap() {
            return Ok(protocol.into());
        }

        // TODO: Add additional formats later
        Err(DiscoveryError::UnsupportedFormat)
    }

    /// Sets the channel configuration used by the protocol.
    pub fn with_channel_config(self, channel_config: ChannelConfig) -> Self {
        match self {
            Self::Pixi(protocol) => Self::Pixi(protocol.with_channel_config(channel_config)),
            Self::CondaBuild(protocol) => {
                Self::CondaBuild(protocol.with_channel_config(channel_config))
            }
        }
    }

    /// Returns the name of the protocol.
    pub fn name(&self) -> &str {
        match self {
            Self::Pixi(_) => "pixi",
            Self::CondaBuild(_) => "conda-build",
        }
    }

    /// Returns a build tool specification for the protocol. This describes how
    /// to acquire the build tool for the specific package.
    pub fn backend_tool(&self) -> ToolSpec {
        match self {
            Self::Pixi(protocol) => protocol.backend_tool(),
            Self::CondaBuild(protocol) => protocol.backend_tool(),
        }
    }

    /// Finish the construction of the protocol and return the protocol object
    pub async fn finish(self, tool: Tool) -> Result<Protocol, FinishError> {
        match self {
            Self::Pixi(protocol) => Ok(Protocol::Pixi(protocol.finish(tool).await?)),
            Self::CondaBuild(protocol) => Ok(Protocol::CondaBuild(protocol.finish(tool))),
        }
    }
}

/// A protocol describes how to communicate with a build backend. A build
/// backend is a tool that is invoked to generate certain output.
///
/// The frontend can support multiple backends, and the protocol is used to
/// determine which backend to use for a given source directory and how to
/// communicate with it.
///
///
/// The [`pixi::Protocol`] protocol is a generic implementation that uses a
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
///
// I think because we mostly have a single variant in use, boxing does not make
// sense here.
#[allow(clippy::large_enum_variant)]
pub enum Protocol {
    Pixi(pixi::Protocol),
    CondaBuild(conda_build::Protocol),
}

impl From<pixi::Protocol> for Protocol {
    fn from(value: pixi::Protocol) -> Self {
        Self::Pixi(value)
    }
}

impl From<conda_build::Protocol> for Protocol {
    fn from(value: conda_build::Protocol) -> Self {
        Self::CondaBuild(value)
    }
}

impl Protocol {
    pub async fn get_conda_metadata(
        &self,
        request: &crate::CondaMetadataRequest,
    ) -> miette::Result<crate::CondaMetadata> {
        match self {
            Self::Pixi(protocol) => protocol.get_conda_metadata(request).await,
            Self::CondaBuild(protocol) => protocol.get_conda_metadata(request),
        }
    }
}
