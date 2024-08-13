use std::path::Path;

use rattler_conda_types::ChannelConfig;

use crate::{
    conda_build::CondaBuildProtocol,
    pixi::PixiProtocol,
    tool::{Tool, ToolSpec},
    CondaMetadata, CondaMetadataRequest,
};

/// A protocol describes how to communicate with a build backend. A build
/// backend is a tool that is invoked to generate certain output.
///
/// The frontend can support multiple backends, and the protocol is used to
/// determine which backend to use for a given source directory and how to
/// communicate with it.
///
/// The [`PixiProtocol`] protocol is a generic implementation that uses a
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
#[derive(Debug)]
pub(crate) enum Protocol {
    /// A pixi project.
    Pixi(PixiProtocol),

    /// A directory containing a `meta.yaml` that can be interpreted by
    /// conda-build.
    CondaBuild(CondaBuildProtocol),
}

impl From<PixiProtocol> for Protocol {
    fn from(value: PixiProtocol) -> Self {
        Self::Pixi(value)
    }
}

impl From<CondaBuildProtocol> for Protocol {
    fn from(value: CondaBuildProtocol) -> Self {
        Self::CondaBuild(value)
    }
}

impl Protocol {
    /// Discovers the protocol for the given source directory.
    pub fn discover(source_dir: &Path) -> miette::Result<Option<Self>> {
        if source_dir.is_file() {
            miette::bail!("source directory must be a directory");
        } else if !source_dir.is_dir() {
            miette::bail!("cannot find source directory '{}'", source_dir.display());
        }

        // Try to discover as a pixi project
        if let Some(protocol) = PixiProtocol::discover(source_dir)? {
            return Ok(Some(protocol.into()));
        }

        // Try to discover as a conda build project
        if let Some(protocol) = CondaBuildProtocol::discover(source_dir)? {
            return Ok(Some(protocol.into()));
        }

        // TODO: Add additional formats later
        Ok(None)
    }

    /// Sets the channel configuration used by the protocol.
    pub fn with_channel_config(self, channel_config: ChannelConfig) -> Self {
        match self {
            Self::Pixi(protocol) => Self::Pixi(protocol),
            Self::CondaBuild(protocol) => {
                Self::CondaBuild(protocol.with_channel_config(channel_config))
            }
        }
    }

    /// Returns the name of the protocol.
    pub fn name(&self) -> &str {
        match self {
            Self::Pixi(_) => "pixi",
            Protocol::CondaBuild(_) => "conda-build",
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

    /// Get the metadata from the source directory.
    pub fn get_conda_metadata(
        &self,
        backend: &Tool,
        request: &CondaMetadataRequest,
    ) -> miette::Result<CondaMetadata> {
        match self {
            Self::Pixi(protocol) => protocol.get_conda_metadata(backend, request),
            Self::CondaBuild(protocol) => protocol.get_conda_metadata(backend, request),
        }
    }
}
