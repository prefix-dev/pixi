use std::str::FromStr;

use rattler_conda_types::NamedChannelOrUrl;
use serde::{de::Error, Deserialize, Deserializer};
use serde_with::serde_as;

/// A channel with an optional priority.
/// If the priority is not specified, it is assumed to be 0.
/// The higher the priority, the more important the channel is.
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
pub struct PrioritizedChannel {
    pub channel: NamedChannelOrUrl,
    pub priority: Option<i32>,
}

impl From<NamedChannelOrUrl> for PrioritizedChannel {
    fn from(value: NamedChannelOrUrl) -> Self {
        Self {
            channel: value,
            priority: None,
        }
    }
}

pub enum TomlPrioritizedChannelStrOrMap {
    Map(PrioritizedChannel),
    Str(NamedChannelOrUrl),
}

impl TomlPrioritizedChannelStrOrMap {
    pub fn into_prioritized_channel(self) -> PrioritizedChannel {
        match self {
            TomlPrioritizedChannelStrOrMap::Map(prioritized_channel) => prioritized_channel,
            TomlPrioritizedChannelStrOrMap::Str(channel) => PrioritizedChannel {
                channel,
                priority: None,
            },
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
                NamedChannelOrUrl::from_str(str)
                    .map_err(serde_untagged::de::Error::custom)
                    .map(TomlPrioritizedChannelStrOrMap::Str)
            })
            .expecting("either a map or a string")
            .deserialize(deserializer)
    }
}

/// Helper so that we can deserialize
/// [`crate::project::manifest::serde::PrioritizedChannel`] from a string or a
/// map.
impl<'de> serde_with::DeserializeAs<'de, PrioritizedChannel> for TomlPrioritizedChannelStrOrMap {
    fn deserialize_as<D>(deserializer: D) -> Result<PrioritizedChannel, D::Error>
    where
        D: Deserializer<'de>,
    {
        let prioritized_channel = TomlPrioritizedChannelStrOrMap::deserialize(deserializer)?;
        Ok(prioritized_channel.into_prioritized_channel())
    }
}
