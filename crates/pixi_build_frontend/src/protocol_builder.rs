use std::path::{Path, PathBuf};

use rattler_conda_types::ChannelConfig;

use crate::{
    conda_build_protocol, pixi_protocol,
    protocol::{DiscoveryError, FinishError},
    rattler_build_protocol,
    tool::ToolCache,
    BackendOverride, BuildFrontendError, Protocol,
};

#[derive(Debug)]
pub(crate) enum ProtocolBuilder {
    /// A pixi project.
    Pixi(pixi_protocol::ProtocolBuilder),

    /// A directory containing a `meta.yaml` that can be interpreted by
    /// conda-build.
    CondaBuild(conda_build_protocol::ProtocolBuilder),

    /// A directory containing a `recipe.yaml` that can be interpreted by
    /// rattler-build.
    RattlerBuild(rattler_build_protocol::ProtocolBuilder),
}

impl From<pixi_protocol::ProtocolBuilder> for ProtocolBuilder {
    fn from(value: pixi_protocol::ProtocolBuilder) -> Self {
        Self::Pixi(value)
    }
}

impl From<conda_build_protocol::ProtocolBuilder> for ProtocolBuilder {
    fn from(value: conda_build_protocol::ProtocolBuilder) -> Self {
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
    pub fn discover(source_dir: &Path) -> Result<Self, DiscoveryError> {
        if source_dir.is_file() {
            return Err(DiscoveryError::NotADirectory);
        } else if !source_dir.is_dir() {
            return Err(DiscoveryError::NotFound(source_dir.to_path_buf()));
        }

        // Try to discover as a rattler-build recipe first
        // and it also should be a `pixi` project
        if let Some(protocol) =
            rattler_build_protocol::ProtocolBuilder::discover(source_dir).unwrap()
        {
            if pixi_protocol::ProtocolBuilder::discover(source_dir)?.is_some() {
                return Ok(protocol.into());
            }
        }

        // Try to discover as a pixi project
        if let Some(protocol) = pixi_protocol::ProtocolBuilder::discover(source_dir)? {
            return Ok(protocol.into());
        }

        // Try to discover as a conda build project
        // Unwrap because error is Infallible
        if let Some(protocol) = conda_build_protocol::ProtocolBuilder::discover(source_dir).unwrap()
        {
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
        tool_cache: &ToolCache,
        build_id: usize,
    ) -> Result<Protocol, BuildFrontendError> {
        match self {
            Self::Pixi(protocol) => Ok(Protocol::Pixi(
                protocol
                    .finish(tool_cache, build_id)
                    .await
                    .map_err(FinishError::Pixi)?,
            )),
            Self::CondaBuild(protocol) => Ok(Protocol::CondaBuild(
                protocol
                    .finish(tool_cache)
                    .await
                    .map_err(FinishError::CondaBuild)?,
            )),
            Self::RattlerBuild(protocol) => Ok(Protocol::RattlerBuild(
                protocol
                    .finish(tool_cache, build_id)
                    .await
                    .map_err(FinishError::RattlerBuild)?,
            )),
        }
    }
}
