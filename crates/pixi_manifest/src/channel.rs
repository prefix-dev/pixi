use std::str::FromStr;

use itertools::Itertools;
use rattler_conda_types::NamedChannelOrUrl;
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use serde_with::serde_as;
use toml_edit::{Table, Value};

/// A channel with an optional priority.
/// If the priority is not specified, it is assumed to be 0.
/// The higher the priority, the more important the channel is.
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct PrioritizedChannel {
    pub channel: NamedChannelOrUrl,
    pub priority: Option<i32>,
}

impl PrioritizedChannel {
    /// The prioritized channels contain a priority, sort on this priority.
    /// Higher priority comes first. [-10, 1, 0 ,2] -> [2, 1, 0, -10]
    pub fn sort_channels_by_priority<'a, I>(
        channels: I,
    ) -> impl Iterator<Item = &'a NamedChannelOrUrl>
    where
        I: IntoIterator<Item = &'a crate::PrioritizedChannel>,
    {
        channels
            .into_iter()
            .sorted_by(|a, b| {
                let a = a.priority.unwrap_or(0);
                let b = b.priority.unwrap_or(0);
                b.cmp(&a)
            })
            .map(|prioritized_channel| &prioritized_channel.channel)
    }
}

impl From<NamedChannelOrUrl> for PrioritizedChannel {
    fn from(value: NamedChannelOrUrl) -> Self {
        Self {
            channel: value,
            priority: None,
        }
    }
}

impl From<(NamedChannelOrUrl, Option<i32>)> for PrioritizedChannel {
    fn from((value, prio): (NamedChannelOrUrl, Option<i32>)) -> Self {
        Self {
            channel: value,
            priority: prio,
        }
    }
}

impl From<PrioritizedChannel> for Value {
    fn from(channel: PrioritizedChannel) -> Self {
        match channel.priority {
            Some(priority) => {
                let mut table = Table::new().into_inline_table();
                table.insert("channel", channel.channel.to_string().into());
                table.insert("priority", i64::from(priority).into());
                Value::InlineTable(table)
            }
            None => Value::String(toml_edit::Formatted::new(channel.channel.to_string())),
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

impl From<PrioritizedChannel> for TomlPrioritizedChannelStrOrMap {
    fn from(channel: PrioritizedChannel) -> Self {
        if let Some(priority) = channel.priority {
            TomlPrioritizedChannelStrOrMap::Map(PrioritizedChannel {
                channel: channel.channel,
                priority: Some(priority),
            })
        } else {
            TomlPrioritizedChannelStrOrMap::Str(channel.channel)
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

impl Serialize for TomlPrioritizedChannelStrOrMap {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            TomlPrioritizedChannelStrOrMap::Map(map) => map.serialize(serializer),
            TomlPrioritizedChannelStrOrMap::Str(str) => str.serialize(serializer),
        }
    }
}

/// Helper so that we can deserialize
/// [`crate::channel::PrioritizedChannel`] from a string or a
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

/// Helper so that we can serialize
/// [`crate::channel::PrioritizedChannel`] to a string or a
/// map.
impl serde_with::SerializeAs<PrioritizedChannel> for TomlPrioritizedChannelStrOrMap {
    fn serialize_as<S>(source: &PrioritizedChannel, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let toml_prioritized_channel: TomlPrioritizedChannelStrOrMap = source.clone().into();
        toml_prioritized_channel.serialize(serializer)
    }
}
