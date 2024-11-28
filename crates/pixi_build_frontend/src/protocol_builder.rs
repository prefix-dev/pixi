use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rattler_conda_types::ChannelConfig;

use crate::{
    conda_protocol, pixi_protocol,
    protocol::{DiscoveryError, FinishError},
    rattler_build_protocol, BackendOverride, BuildFrontendError, Protocol, ToolContext,
};

/// Configuration to enable or disable certain protocols discovery.
#[derive(Debug)]
pub struct EnabledProtocols {
    /// Enable the rattler-build protocol.
    pub enable_rattler_build: bool,
    /// Enable the pixi protocol.
    pub enable_pixi: bool,
    /// Enable the conda-build protocol.
    pub enable_conda_build: bool,
}

impl Default for EnabledProtocols {
    /// Create a new `EnabledProtocols` with all protocols enabled.
    fn default() -> Self {
        Self {
            enable_rattler_build: true,
            enable_pixi: true,
            enable_conda_build: true,
        }
    }
}

#[derive(Debug)]
pub(crate) enum ProtocolBuilder {
    /// A pixi project.
    Pixi(pixi_protocol::ProtocolBuilder),

    /// A directory containing a `meta.yaml` that can be interpreted by
    /// conda-build.
    CondaBuild(conda_protocol::ProtocolBuilder),

    /// A directory containing a `recipe.yaml` that can be built with
    /// rattler-build.
    RattlerBuild(rattler_build_protocol::ProtocolBuilder),
}

impl From<pixi_protocol::ProtocolBuilder> for ProtocolBuilder {
    fn from(value: pixi_protocol::ProtocolBuilder) -> Self {
        Self::Pixi(value)
    }
}

impl From<conda_protocol::ProtocolBuilder> for ProtocolBuilder {
    fn from(value: conda_protocol::ProtocolBuilder) -> Self {
        Self::CondaBuild(value)
    }
}

impl From<rattler_build_protocol::ProtocolBuilder> for ProtocolBuilder {
    fn from(value: rattler_build_protocol::ProtocolBuilder) -> Self {
        Self::RattlerBuild(value)
    }
}

impl ProtocolBuilder {
    /// Discovers the protocol for the given source directory.
    pub fn discover(
        source_dir: &Path,
        enabled_protocols: &EnabledProtocols,
    ) -> Result<Self, DiscoveryError> {
        if source_dir.is_file() {
            return Err(DiscoveryError::NotADirectory);
        } else if !source_dir.is_dir() {
            return Err(DiscoveryError::NotFound(source_dir.to_path_buf()));
        }

        // Try to discover as a rattler-build recipe first
        if enabled_protocols.enable_rattler_build {
            if let Some(protocol) = rattler_build_protocol::ProtocolBuilder::discover(source_dir)? {
                return Ok(protocol.into());
            }
        }

        // Try to discover as a conda build project
        if enabled_protocols.enable_conda_build {
            // Unwrap as the error is infallible
            if let Some(protocol) = conda_protocol::ProtocolBuilder::discover(source_dir).unwrap() {
                return Ok(protocol.into());
            }
        }

        // Try to discover as a pixi project
        if enabled_protocols.enable_pixi {
            if let Some(protocol) = pixi_protocol::ProtocolBuilder::discover(source_dir)? {
                return Ok(protocol.into());
            }
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
            Self::RattlerBuild(protocol) => {
                Self::RattlerBuild(protocol.with_channel_config(channel_config))
            }
        }
    }

    pub(crate) fn with_backend_override(self, backend: Option<BackendOverride>) -> Self {
        match self {
            Self::Pixi(protocol) => Self::Pixi(protocol.with_backend_override(backend)),
            Self::CondaBuild(protocol) => Self::CondaBuild(protocol.with_backend_override(backend)),
            Self::RattlerBuild(protocol) => {
                Self::RattlerBuild(protocol.with_backend_override(backend))
            }
        }
    }

    /// Sets the cache directory to use for any caching.
    pub fn with_opt_cache_dir(self, cache_directory: Option<PathBuf>) -> Self {
        match self {
            Self::Pixi(protocol) => Self::Pixi(protocol.with_opt_cache_dir(cache_directory)),
            Self::CondaBuild(protocol) => {
                Self::CondaBuild(protocol.with_opt_cache_dir(cache_directory))
            }
            Self::RattlerBuild(protocol) => {
                Self::RattlerBuild(protocol.with_opt_cache_dir(cache_directory))
            }
        }
    }

    /// Returns the name of the protocol.
    pub fn name(&self) -> &str {
        match self {
            Self::Pixi(_) => "pixi",
            Self::CondaBuild(_) => "conda-build",
            Self::RattlerBuild(_) => "rattler-build",
        }
    }

    /// Finish the construction of the protocol and return the protocol object
    pub async fn finish(
        self,
        tool_context: Arc<ToolContext>,
        build_id: usize,
    ) -> Result<Protocol, BuildFrontendError> {
        match self {
            Self::Pixi(protocol) => Ok(protocol
                .finish(tool_context, build_id)
                .await
                .map_err(FinishError::Pixi)?
                .into()),
            Self::CondaBuild(protocol) => Ok(protocol
                .finish(tool_context, build_id)
                .await
                .map_err(FinishError::CondaBuild)?
                .into()),
            Self::RattlerBuild(protocol) => Ok(protocol
                .finish(tool_context, build_id)
                .await
                .map_err(FinishError::RattlerBuild)?
                .into()),
        }
    }
}
