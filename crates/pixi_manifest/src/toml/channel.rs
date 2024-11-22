use std::str::FromStr;

use rattler_conda_types::NamedChannelOrUrl;
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};

use crate::PrioritizedChannel;

/// Layout of a prioritized channel in a toml file.
///
/// Supports the following formats:
///
/// ```toml
/// channel = "some-channel"
/// channel = "https://prefix.dev/some-channel"
/// channel = { channel = "some-channel", priority = 10 }
/// ```
#[derive(Debug)]
pub enum TomlPrioritizedChannel {
    Map(PrioritizedChannel),
    Str(NamedChannelOrUrl),
}

impl From<TomlPrioritizedChannel> for PrioritizedChannel {
    fn from(channel: TomlPrioritizedChannel) -> Self {
        match channel {
            TomlPrioritizedChannel::Map(prioritized_channel) => prioritized_channel,
            TomlPrioritizedChannel::Str(channel) => PrioritizedChannel {
                channel,
                priority: None,
            },
        }
    }
}

impl From<PrioritizedChannel> for TomlPrioritizedChannel {
    fn from(channel: PrioritizedChannel) -> Self {
        if let Some(priority) = channel.priority {
            TomlPrioritizedChannel::Map(PrioritizedChannel {
                channel: channel.channel,
                priority: Some(priority),
            })
        } else {
            TomlPrioritizedChannel::Str(channel.channel)
        }
    }
}

impl<'de> Deserialize<'de> for TomlPrioritizedChannel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .map(|map| map.deserialize().map(TomlPrioritizedChannel::Map))
            .string(|str| {
                NamedChannelOrUrl::from_str(str)
                    .map_err(serde_untagged::de::Error::custom)
                    .map(TomlPrioritizedChannel::Str)
            })
            .expecting("either a map or a string")
            .deserialize(deserializer)
    }
}

impl Serialize for TomlPrioritizedChannel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            TomlPrioritizedChannel::Map(map) => map.serialize(serializer),
            TomlPrioritizedChannel::Str(str) => str.serialize(serializer),
        }
    }
}

/// Helper so that we can deserialize [`crate::channel::PrioritizedChannel`]
/// from a string or a map.
impl<'de> serde_with::DeserializeAs<'de, PrioritizedChannel> for TomlPrioritizedChannel {
    fn deserialize_as<D>(deserializer: D) -> Result<PrioritizedChannel, D::Error>
    where
        D: Deserializer<'de>,
    {
        let prioritized_channel = TomlPrioritizedChannel::deserialize(deserializer)?;
        Ok(prioritized_channel.into())
    }
}

/// Helper so that we can serialize [`crate::channel::PrioritizedChannel`] to a
/// string or a map.
impl serde_with::SerializeAs<PrioritizedChannel> for TomlPrioritizedChannel {
    fn serialize_as<S>(source: &PrioritizedChannel, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let toml_prioritized_channel: TomlPrioritizedChannel = source.clone().into();
        toml_prioritized_channel.serialize(serializer)
    }
}
