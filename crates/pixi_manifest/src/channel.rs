use itertools::Itertools;
use pixi_spec::ExcludeNewer;
use rattler_conda_types::NamedChannelOrUrl;
use toml_edit::{Table, Value};

/// A channel with an optional priority.
/// If the priority is not specified, it is assumed to be 0.
/// The higher the priority, the more important the channel is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrioritizedChannel {
    pub channel: NamedChannelOrUrl,
    pub priority: Option<i32>,
    pub exclude_newer: Option<ExcludeNewer>,
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
            exclude_newer: None,
        }
    }
}

impl From<(NamedChannelOrUrl, Option<i32>)> for PrioritizedChannel {
    fn from((value, prio): (NamedChannelOrUrl, Option<i32>)) -> Self {
        Self {
            channel: value,
            priority: prio,
            exclude_newer: None,
        }
    }
}

impl From<PrioritizedChannel> for Value {
    fn from(channel: PrioritizedChannel) -> Self {
        match (channel.priority, channel.exclude_newer) {
            (Some(priority), exclude_newer) => {
                let mut table = Table::new().into_inline_table();
                table.insert("channel", channel.channel.to_string().into());
                table.insert("priority", i64::from(priority).into());
                if let Some(exclude_newer) = exclude_newer {
                    table.insert("exclude-newer", exclude_newer.to_string().into());
                }
                Value::InlineTable(table)
            }
            (None, Some(exclude_newer)) => {
                let mut table = Table::new().into_inline_table();
                table.insert("channel", channel.channel.to_string().into());
                table.insert("exclude-newer", exclude_newer.to_string().into());
                Value::InlineTable(table)
            }
            (None, None) => Value::String(toml_edit::Formatted::new(channel.channel.to_string())),
        }
    }
}
