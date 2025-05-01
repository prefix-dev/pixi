//! This module is the main entry
use std::{path::PathBuf, sync::Arc};

use miette::Diagnostic;
use rattler_conda_types::ChannelConfig;

use crate::{
    EnabledProtocols, SetupRequest, ToolContext, protocol, protocol_builder::ProtocolBuilder,
    protocols::JsonRPCBuildProtocol,
};

/// The frontend for building packages.
pub struct BuildFrontend {
    /// The cache for tools. This is used to avoid re-installing tools.
    tool_context: Arc<ToolContext>,

    /// The channel configuration used by the frontend
    channel_config: ChannelConfig,

    /// The cache directory to use or `None` to use the default cache directory.
    cache_dir: Option<PathBuf>,

    /// The configuration to use when enabling the protocols.
    enabled_protocols: EnabledProtocols,
}

impl Default for BuildFrontend {
    fn default() -> Self {
        Self {
            tool_context: Arc::new(ToolContext::default()),
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::new()),
            cache_dir: None,
            enabled_protocols: EnabledProtocols::default(),
        }
    }
}

#[derive(thiserror::Error, Debug, Diagnostic)]
pub enum BuildFrontendError {
    /// Error while discovering the pixi.toml
    #[error(transparent)]
    #[diagnostic(transparent)]
    DiscoveringManifest(#[from] protocol::DiscoveryError),
    /// Error from the build protocol.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Protocol(#[from] protocol::FinishError),
    /// Error discovering system-tool
    #[error("error discovering system-tool")]
    ToolError(#[from] which::Error),
}

impl BuildFrontend {
    /// Specify the channel configuration
    pub fn with_channel_config(self, channel_config: ChannelConfig) -> Self {
        Self {
            channel_config,
            ..self
        }
    }

    /// Returns the channel config of the frontend
    pub fn channel_config(&self) -> &ChannelConfig {
        &self.channel_config
    }

    /// Optionally sets the cache directory the backend should use.
    pub fn with_opt_cache_dir(self, cache_dir: Option<PathBuf>) -> Self {
        Self { cache_dir, ..self }
    }

    /// Sets the cache directory the backend should use.
    pub fn with_cache_dir(self, cache_dir: PathBuf) -> Self {
        Self {
            cache_dir: Some(cache_dir),
            ..self
        }
    }

    /// Sets the tool context
    pub fn with_tool_context(self, context: Arc<ToolContext>) -> Self {
        Self {
            tool_context: context,
            ..self
        }
    }

    /// Sets the enabling protocols.
    pub fn with_enabled_protocols(self, enabled_protocols: EnabledProtocols) -> Self {
        Self {
            enabled_protocols,
            ..self
        }
    }

    /// Constructs a new [`JsonRPCBuildProtocol`] for the given request. This object can be
    /// used to build the package.
    pub async fn setup_protocol(
        &self,
        request: SetupRequest,
    ) -> Result<JsonRPCBuildProtocol, BuildFrontendError> {
        // Determine the build protocol to use for the source directory.
        let protocol = ProtocolBuilder::discover(&request.source_dir, &self.enabled_protocols)?
            .with_channel_config(self.channel_config.clone())
            .with_opt_cache_dir(self.cache_dir.clone());

        tracing::info!(
            "discovered a {} source package at {}",
            protocol.name(),
            request.source_dir.display()
        );

        protocol
            .with_backend_override(request.build_tool_override)
            .finish(self.tool_context.clone(), request.build_id)
            .await
    }
}
