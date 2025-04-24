use std::str::FromStr;

use crate::PrioritizedChannel;
use pixi_toml::TomlFromStr;
use rattler_conda_types::NamedChannelOrUrl;
use serde::{Serialize, Serializer};
use toml_span::de_helpers::expected;
use toml_span::{DeserError, ErrorKind, Value, de_helpers::TableHelper, value::ValueInner};

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

impl Serialize for PrioritizedChannel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        TomlPrioritizedChannel::from(self.clone()).serialize(serializer)
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlPrioritizedChannel {
    fn deserialize(value: &mut toml_span::Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(name) => {
                let name = NamedChannelOrUrl::from_str(&name).map_err(|e| toml_span::Error {
                    kind: ErrorKind::Custom(e.to_string().into()),
                    span: value.span,
                    line_info: None,
                })?;
                Ok(TomlPrioritizedChannel::Str(name))
            }
            inner @ ValueInner::Table(_) => {
                let mut th = TableHelper::new(&mut toml_span::Value::with_span(inner, value.span))?;
                let channel = th.required::<TomlFromStr<_>>("channel")?;
                let priority = th.optional("priority");
                th.finalize(None)?;
                Ok(TomlPrioritizedChannel::Map(PrioritizedChannel {
                    channel: channel.into_inner(),
                    priority,
                }))
            }
            other => Err(expected("a string or table", other, value.span).into()),
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for PrioritizedChannel {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        <TomlPrioritizedChannel as toml_span::Deserialize>::deserialize(value).map(Into::into)
    }
}

#[cfg(test)]
mod test {
    use insta::{assert_debug_snapshot, assert_snapshot};
    use toml_span::Value;

    use super::*;
    use crate::{toml::FromTomlStr, utils::test_utils::format_parse_error};

    #[allow(dead_code)]
    #[derive(Debug)]
    struct TopLevel {
        channel: TomlPrioritizedChannel,
    }

    impl<'de> toml_span::Deserialize<'de> for TopLevel {
        fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
            let mut th = TableHelper::new(value)?;
            let channel = th.required("channel")?;
            th.finalize(None)?;
            Ok(TopLevel { channel })
        }
    }

    #[test]
    fn test_map() {
        let channel = TopLevel::from_toml_str(
            r#"
        channel = { channel = "some-channel" }
        "#,
        )
        .unwrap();
        assert_debug_snapshot!(channel, @r###"
        TopLevel {
            channel: Map(
                PrioritizedChannel {
                    channel: Name(
                        "some-channel",
                    ),
                    priority: None,
                },
            ),
        }
        "###);
    }

    #[test]
    fn test_with_priority() {
        let channel = TopLevel::from_toml_str(
            r#"
        channel = { channel = "some-channel", priority = 10 }
        "#,
        )
        .unwrap();
        assert_debug_snapshot!(channel, @r###"
        TopLevel {
            channel: Map(
                PrioritizedChannel {
                    channel: Name(
                        "some-channel",
                    ),
                    priority: Some(
                        10,
                    ),
                },
            ),
        }
        "###);
    }

    #[test]
    fn test_without_name() {
        let input = r#"
        channel = { priority = 10 }
        "#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        assert_snapshot!(format_parse_error(input, error), @r###"
         × missing field 'channel' in table
          ╭─[pixi.toml:2:19]
        1 │
        2 │         channel = { priority = 10 }
          ·                   ─────────────────
        3 │
          ╰────
        "###);
    }
}
