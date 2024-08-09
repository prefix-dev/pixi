use std::path::PathBuf;

use crate::{get_cache_dir, Config};
use rattler_repodata_gateway::{ChannelConfig, Gateway, SourceConfig};
use reqwest_middleware::ClientWithMiddleware;

/// Converts a [`Config`] into a rattler [`ChannelConfig`]
pub fn from_pixi_config(config: &Config) -> ChannelConfig {
    let default_source_config = config
        .repodata_config
        .as_ref()
        .map(|config| SourceConfig {
            jlap_enabled: !config.disable_jlap.unwrap_or(false),
            zstd_enabled: !config.disable_zstd.unwrap_or(false),
            bz2_enabled: !config.disable_bzip2.unwrap_or(false),
            cache_action: Default::default(),
        })
        .unwrap_or_default();

    ChannelConfig {
        default: default_source_config,
        per_channel: Default::default(),
    }
}

/// Constructs a [`Gateway`] from a [`ClientWithMiddleware`] and a [`Config`]
pub fn new_gateway(client: ClientWithMiddleware, config: Config) -> Gateway {
    // Determine the cache directory and fall back to sane defaults otherwise.
    let cache_dir = get_cache_dir().unwrap_or_else(|e| {
        tracing::error!("failed to determine repodata cache directory: {e}");
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("./"))
    });

    // Construct the gateway
    Gateway::builder()
        .with_client(client)
        .with_cache_dir(cache_dir.join("repodata"))
        .with_channel_config(config.into())
        .finish()
}
