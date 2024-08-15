use crate::protocol::{DiscoveryError, FinishError};
use crate::tool::Tool;
use crate::{conda_build, pixi, Protocol, ToolSpec};
use rattler_conda_types::ChannelConfig;
use std::path::Path;

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
