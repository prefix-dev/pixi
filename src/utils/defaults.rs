use rattler_conda_types::ChannelConfig;

pub fn default_channel_config() -> ChannelConfig {
    ChannelConfig::default_with_root_dir(
        std::env::current_dir().expect("Could not retrieve the current directory"),
    )
}
