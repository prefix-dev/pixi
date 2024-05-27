use crate::{config, project::Project};
use rattler_repodata_gateway::{ChannelConfig, Gateway, SourceConfig};
use std::path::PathBuf;

impl Project {
    /// Returns the [`Gateway`] used by this project.
    pub fn repodata_gateway(&self) -> &Gateway {
        self.repodata_gateway.get_or_init(|| {
            // Determine the cache directory and fall back to sane defaults otherwise.
            let cache_dir = config::get_cache_dir().unwrap_or_else(|e| {
                tracing::error!("failed to determine repodata cache directory: {e}");
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("./"))
            });

            // Determine the default configuration from the config
            let default_source_config = self
                .config
                .repodata_config
                .as_ref()
                .map(|config| SourceConfig {
                    jlap_enabled: !config.disable_jlap.unwrap_or(false),
                    zstd_enabled: !config.disable_zstd.unwrap_or(false),
                    bz2_enabled: !config.disable_bzip2.unwrap_or(false),
                    cache_action: Default::default(),
                })
                .unwrap_or_default();

            // Construct the gateway
            Gateway::builder()
                .with_client(self.authenticated_client().clone())
                .with_cache_dir(cache_dir.join("repodata"))
                .with_channel_config(ChannelConfig {
                    default: default_source_config,
                    per_channel: Default::default(),
                })
                .finish()
        })
    }
}
