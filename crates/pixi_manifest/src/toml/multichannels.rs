//! The `[workspace.multichannels]` table of a pixi manifest.

use std::str::FromStr;

use indexmap::IndexMap;
use rattler_conda_types::NamedChannelOrUrl;
use toml_span::{DeserError, ErrorKind, Span, Value, de_helpers::expected, value::ValueInner};

/// The definitions of multichannels, by name. Each entry lists the channels
/// of the multichannel in order of preference.
// TODO: replace with `rattler_conda_types::MultiChannels` once it is released.
pub type MultiChannels = IndexMap<String, Vec<NamedChannelOrUrl>>;

/// The TOML representation of the `[workspace.multichannels]` table:
///
/// ```toml
/// [workspace.multichannels]
/// defaults = ["https://repo.anaconda.com/pkgs/main", "https://repo.anaconda.com/pkgs/r"]
/// ```
///
/// Deserializing validates the definitions: every name must be a valid
/// multichannel name, every multichannel must contain at least one channel,
/// a channel must not be named after a multichannel and no channel may occur
/// more than once in the same multichannel.
#[derive(Debug, Clone, Default)]
pub struct TomlMultiChannels {
    pub multichannels: MultiChannels,
    pub span: Span,
}

/// Returns true if `name` can be used as the name of a multichannel: it must
/// be non-empty and must not contain a path separator, a `:`, a `,`, a `=` or
/// whitespace, so that it cannot be mistaken for a channel URL or path.
// TODO: replace with `rattler_conda_types::is_valid_multi_channel_name` once
// it is released.
fn is_valid_multichannel_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\', ':', ',', '='])
        && !name.chars().any(char::is_whitespace)
}

fn custom_error(message: String, span: Span) -> toml_span::Error {
    toml_span::Error {
        kind: ErrorKind::Custom(message.into()),
        span,
        line_info: None,
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlMultiChannels {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let span = value.span;
        let table = match value.take() {
            ValueInner::Table(table) => table,
            inner => return Err(expected("a table", inner, span).into()),
        };

        let mut errors = DeserError { errors: vec![] };
        let names: Vec<String> = table.keys().map(|key| key.name.to_string()).collect();
        let mut multichannels = MultiChannels::new();
        for (key, mut entry) in table {
            let name = key.name.into_owned();
            if !is_valid_multichannel_name(&name) {
                errors.errors.push(custom_error(
                    format!(
                        "invalid multichannel name '{name}', it must be non-empty and must not contain '/', '\\', ':', ',', '=' or whitespace"
                    ),
                    key.span,
                ));
                continue;
            }

            let entry_span = entry.span;
            let members = match entry.take() {
                ValueInner::Array(members) => members,
                inner => {
                    errors
                        .errors
                        .push(expected("an array of channels", inner, entry_span));
                    continue;
                }
            };
            if members.is_empty() {
                errors.errors.push(custom_error(
                    format!("multichannel '{name}' does not contain any channels"),
                    entry_span,
                ));
                continue;
            }

            let mut channels: Vec<NamedChannelOrUrl> = Vec::with_capacity(members.len());
            for mut member in members {
                let member_span = member.span;
                let channel = match member.take() {
                    ValueInner::String(channel) => channel,
                    inner => {
                        errors
                            .errors
                            .push(expected("a channel name or URL", inner, member_span));
                        continue;
                    }
                };
                let channel = match NamedChannelOrUrl::from_str(&channel) {
                    Ok(channel) => channel,
                    Err(e) => {
                        errors.errors.push(custom_error(e.to_string(), member_span));
                        continue;
                    }
                };
                if let NamedChannelOrUrl::Name(member_name) = &channel
                    && names.contains(member_name)
                {
                    errors.errors.push(custom_error(
                        format!(
                            "multichannel '{name}' cannot contain another multichannel ('{member_name}')"
                        ),
                        member_span,
                    ));
                    continue;
                }
                if channels.contains(&channel) {
                    errors.errors.push(custom_error(
                        format!("multichannel '{name}' contains '{channel}' more than once"),
                        member_span,
                    ));
                    continue;
                }
                channels.push(channel);
            }
            multichannels.insert(name, channels);
        }

        if errors.errors.is_empty() {
            Ok(Self {
                multichannels,
                span,
            })
        } else {
            Err(errors)
        }
    }
}
