use crate::Config;
use rattler_repodata_gateway::{ChannelConfig, SourceConfig};

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
