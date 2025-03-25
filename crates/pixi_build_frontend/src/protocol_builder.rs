use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::Arc,
};

use rattler_conda_types::ChannelConfig;

use crate::{
    backend_override::BackendOverride,
    pixi_protocol,
    protocol::{DiscoveryError, FinishError},
    protocols::JsonRPCBuildProtocol,
    rattler_build_protocol, BuildFrontendError, ToolContext,
};

/// Configuration to enable or disable certain protocols discovery.
#[derive(Debug)]
pub struct EnabledProtocols {
    /// Enable the rattler-build protocol.
    pub enable_rattler_build: bool,
    /// Enable the pixi protocol.
    pub enable_pixi: bool,
}

impl Default for EnabledProtocols {
    /// Create a new `EnabledProtocols` with all protocols enabled.
    fn default() -> Self {
        Self {
            enable_rattler_build: true,
            enable_pixi: true,
        }
    }
}

#[derive(Debug)]
// for some reason, the clippy calculates wrong the size
// for this enum variants, so we need to disable the warning
#[allow(clippy::large_enum_variant)]
pub(crate) enum ProtocolBuilder {
    /// A pixi project.
    Pixi(pixi_protocol::ProtocolBuilder),

    /// A directory containing a `recipe.yaml` that can be built with
    /// rattler-build.
    RattlerBuild(rattler_build_protocol::ProtocolBuilder),
}

impl From<pixi_protocol::ProtocolBuilder> for ProtocolBuilder {
    fn from(value: pixi_protocol::ProtocolBuilder) -> Self {
        Self::Pixi(value)
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
        source_path: &Path,
        enabled_protocols: &EnabledProtocols,
    ) -> Result<Self, DiscoveryError> {
        if !source_path.exists() {
            return Err(DiscoveryError::NotFound(source_path.to_path_buf()));
        }

        let source_file_name = source_path.file_name().and_then(OsStr::to_str);

        // If the user explicitly asked for a recipe.yaml file
        if matches!(source_file_name, Some("recipe.yaml" | "recipe.yml")) {
            let source_dir = source_path
                .parent()
                .expect("the recipe must live somewhere");
            return if enabled_protocols.enable_rattler_build {
                Ok(rattler_build_protocol::ProtocolBuilder::new(
                    source_dir.to_path_buf(),
                    source_dir.to_path_buf(),
                )
                .into())
            } else {
                Err(DiscoveryError::UnsupportedFormat)
            };
        }

        // Try to discover as a pixi project
        if enabled_protocols.enable_pixi {
            if let Some(protocol) = pixi_protocol::ProtocolBuilder::discover(source_path)? {
                return Ok(protocol.into());
            }
        }

        // Try to discover as a rattler-build recipe
        if enabled_protocols.enable_rattler_build {
            if let Some(protocol) = rattler_build_protocol::ProtocolBuilder::discover(source_path)?
            {
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
            Self::RattlerBuild(protocol) => {
                Self::RattlerBuild(protocol.with_channel_config(channel_config))
            }
        }
    }

    pub(crate) fn with_backend_override(self, backend_override: Option<BackendOverride>) -> Self {
        if let Some(backend_override) = backend_override {
            match self {
                Self::Pixi(protocol) => {
                    Self::Pixi(protocol.with_backend_override(backend_override))
                }
                Self::RattlerBuild(protocol) => {
                    Self::RattlerBuild(protocol.with_backend_override(backend_override))
                }
            }
        } else {
            self
        }
    }

    /// Sets the cache directory to use for any caching.
    pub fn with_opt_cache_dir(self, cache_directory: Option<PathBuf>) -> Self {
        match self {
            Self::Pixi(protocol) => Self::Pixi(protocol.with_opt_cache_dir(cache_directory)),
            Self::RattlerBuild(protocol) => {
                Self::RattlerBuild(protocol.with_opt_cache_dir(cache_directory))
            }
        }
    }

    /// Returns the name of the protocol.
    pub fn name(&self) -> &str {
        match self {
            Self::Pixi(_) => "pixi",
            Self::RattlerBuild(_) => "rattler-build",
        }
    }

    /// Finish the construction of the protocol and return the protocol object
    pub async fn finish(
        self,
        tool_context: Arc<ToolContext>,
        build_id: usize,
    ) -> Result<JsonRPCBuildProtocol, BuildFrontendError> {
        match self {
            Self::Pixi(protocol) => Ok(protocol
                .finish(tool_context, build_id)
                .await
                .map_err(FinishError::Pixi)?),
            Self::RattlerBuild(protocol) => Ok(protocol
                .finish(tool_context, build_id)
                .await
                .map_err(FinishError::RattlerBuild)?),
        }
    }
}
