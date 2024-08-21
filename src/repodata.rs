use rattler_conda_types::Channel;

/// Returns a friendly name for the specified channel.
pub(crate) fn friendly_channel_name(channel: &Channel) -> String {
    channel
        .name
        .as_ref()
        .map(String::from)
        .unwrap_or_else(|| channel.canonical_name())
}
