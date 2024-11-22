//! This module is the main entry
use std::{path::PathBuf, sync::Arc};

use miette::Diagnostic;
use rattler_conda_types::ChannelConfig;

use crate::{
    protocol,
    protocol_builder::{EnabledProtocols, ProtocolBuilder},
    tool::{ToolCache, ToolContext},
    Protocol, SetupRequest,
};

/// The frontend for building packages.
pub struct BuildFrontend {
    /// The cache for tools. This is used to avoid re-installing tools.
    tool_cache: Arc<ToolCache>,

    /// The channel configuration used by the frontend
    channel_config: ChannelConfig,

    /// The cache directory to use or `None` to use the default cache directory.
    cache_dir: Option<PathBuf>,

    /// The configuration to use when discovering the protocol.
    discovery_config: EnabledProtocols,
}

impl Default for BuildFrontend {
    fn default() -> Self {
        Self {
            tool_cache: Arc::new(ToolCache::new()),
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::new()),
            cache_dir: None,
            discovery_config: EnabledProtocols::default(),
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
    pub fn with_tool_context(self, context: ToolContext) -> Self {
        let tool_cache = ToolCache {
            cache: self.tool_cache.cache.clone(),
            context,
        };

        Self {
            tool_cache: tool_cache.into(),
            ..self
        }
    }

    /// Sets the discovery config.
    pub fn with_discovery_config(self, discovery_config: EnabledProtocols) -> Self {
        Self {
            discovery_config,
            ..self
        }
    }

    /// Constructs a new [`Protocol`] for the given request. This object can be
    /// used to build the package.
    pub async fn setup_protocol(
        &self,
        request: SetupRequest,
    ) -> Result<Protocol, BuildFrontendError> {
        // Determine the build protocol to use for the source directory.
        let protocol = ProtocolBuilder::discover(&request.source_dir, &self.discovery_config)?
            .with_channel_config(self.channel_config.clone())
            .with_opt_cache_dir(self.cache_dir.clone());

        tracing::info!(
            "discovered a {} source package at {}",
            protocol.name(),
            request.source_dir.display()
        );

        protocol
            .with_backend_override(request.build_tool_override)
            .finish(&self.tool_cache, request.build_id)
            .await
    }
}
