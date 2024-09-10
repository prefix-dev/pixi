//! This module is the main entry
use crate::protocol_builder::ProtocolBuilder;
use crate::tool::ToolCache;
use crate::{protocol, Protocol, SetupRequest};
use rattler_conda_types::ChannelConfig;
use std::path::PathBuf;
use std::sync::Arc;

/// The frontend for building packages.
pub struct BuildFrontend {
    /// The cache for tools. This is used to avoid re-installing tools.
    tool_cache: Arc<ToolCache>,

    /// The channel configuration used by the frontend
    channel_config: ChannelConfig,
}

impl Default for BuildFrontend {
    fn default() -> Self {
        Self {
            tool_cache: Arc::new(ToolCache::new()),
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::new()),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum BuildFrontendError {
    /// Error while discovering the pixi.toml
    #[error(transparent)]
    DiscoveringManifest(#[from] protocol::DiscoveryError),
    /// Error from the build protocol.
    #[error(transparent)]
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

    /// Construcst a new [`Builder`] for the given request. This object can be
    /// used to build the package.
    pub async fn setup_protocol(
        &self,
        request: SetupRequest,
    ) -> Result<Protocol, BuildFrontendError> {
        // Determine the build protocol to use for the source directory.
        let protocol = ProtocolBuilder::discover(&request.source_dir)?
            .with_channel_config(self.channel_config.clone());

        tracing::info!(
            "discovered a {} source package at {}",
            protocol.name(),
            request.source_dir.display()
        );

        // Instantiate the build tool.
        let tool_spec = request
            .build_tool_overrides
            .into_spec()
            .unwrap_or(protocol.backend_tool());
        let tool = self.tool_cache.instantiate(&tool_spec)?;

        Ok(protocol.finish(tool).await?)
    }
}
