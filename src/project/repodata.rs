use crate::project::Project;

use rattler_repodata_gateway::{ChannelConfig, Gateway};
use std::path::PathBuf;

impl Project {
    /// Returns the [`Gateway`] used by this project.
    pub fn repodata_gateway(&self) -> &Gateway {
        self.repodata_gateway.get_or_init(|| {
            // Determine the cache directory and fall back to sane defaults otherwise.
            let cache_dir = pixi_config::get_cache_dir().unwrap_or_else(|e| {
                tracing::error!("failed to determine repodata cache directory: {e}");
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("./"))
            });

            // Construct the gateway
            Gateway::builder()
                .with_client(self.authenticated_client().clone())
                .with_cache_dir(cache_dir.join("repodata"))
                .with_channel_config(ChannelConfig::from(&self.config))
                .finish()
        })
    }
}
