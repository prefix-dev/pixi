use rattler_conda_types::{Channel, ChannelConfig};
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use serde_with::serde_as;
use std::borrow::Cow;

use crate::utils::default_channel_config;

/// A channel with an optional priority.
/// If the priority is not specified, it is assumed to be 0.
/// The higher the priority, the more important the channel is.
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
pub struct PrioritizedChannel {
    #[serde_as(as = "ChannelStr")]
    pub channel: Channel,
    pub priority: Option<i32>,
}

impl PrioritizedChannel {
    pub fn from_channel(channel: Channel) -> Self {
        Self {
            channel,
            priority: None,
        }
    }

    /// If channel base is part of the default config, returns the name otherwise the base url
    pub fn to_name_or_url(&self) -> String {
        if self
            .channel
            .base_url
            .as_str()
            .contains(default_channel_config().channel_alias.as_str())
        {
            self.channel.name().to_string()
        } else {
            self.channel.base_url.to_string()
        }
    }
}

pub enum TomlPrioritizedChannelStrOrMap {
    Map(PrioritizedChannel),
    Str(Channel),
}

impl TomlPrioritizedChannelStrOrMap {
    pub fn into_prioritized_channel(self) -> PrioritizedChannel {
        match self {
            TomlPrioritizedChannelStrOrMap::Map(prioritized_channel) => prioritized_channel,
            TomlPrioritizedChannelStrOrMap::Str(channel) => {
                PrioritizedChannel::from_channel(channel)
            }
        }
    }
}

impl<'de> Deserialize<'de> for TomlPrioritizedChannelStrOrMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .map(|map| map.deserialize().map(TomlPrioritizedChannelStrOrMap::Map))
            .string(|str| {
                Channel::from_str(str, &default_channel_config())
                    .map(TomlPrioritizedChannelStrOrMap::Str)
                    .map_err(serde::de::Error::custom)
            })
            .expecting("either a map or a string")
            .deserialize(deserializer)
    }
}

/// Helper so that we can deserialize [`crate::project::manifest::serde::PrioritizedChannel`] from a string or a map.
impl<'de> serde_with::DeserializeAs<'de, PrioritizedChannel> for TomlPrioritizedChannelStrOrMap {
    fn deserialize_as<D>(deserializer: D) -> Result<PrioritizedChannel, D::Error>
    where
        D: Deserializer<'de>,
    {
        let prioritized_channel = TomlPrioritizedChannelStrOrMap::deserialize(deserializer)?;
        Ok(prioritized_channel.into_prioritized_channel())
    }
}

pub struct ChannelStr;

/// Required so we can deserialize a channel from a string as we need to inject the [`ChannelConfig`]
impl<'de> serde_with::DeserializeAs<'de, Channel> for ChannelStr {
    fn deserialize_as<D>(deserializer: D) -> Result<Channel, D::Error>
    where
        D: Deserializer<'de>,
    {
        let channel_str = Cow::<str>::deserialize(deserializer)?;
        // TODO find a way to insert the root dir here (based on the `pixi.toml` file location)
        let channel_config = ChannelConfig::default_with_root_dir(
            std::env::current_dir().expect("Could not retrieve the current directory"),
        );
        Channel::from_str(channel_str, &channel_config).map_err(D::Error::custom)
    }
}
