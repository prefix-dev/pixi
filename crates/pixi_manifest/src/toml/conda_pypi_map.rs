//! TOML deserialization for `[workspace.conda-pypi-map]`.
//!
//! The field accepts `false` (disable all lookups) or a per-channel table.
//! Each channel value is either a bare location string, `false` (disable
//! lookups for that channel) or a table with `location`, inline `mapping`
//! entries, `mode` and `cache-ttl`.

use std::collections::HashMap;

use pixi_toml::{TomlEnum, TomlHashMap, custom_error};
use rattler_conda_types::NamedChannelOrUrl;
use toml_span::{
    DeserError, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

use crate::workspace::{
    CondaPypiMap, CondaPypiMapEntry, CondaPypiMapMode, CondaPypiMapSpec, MappingLocationSpec,
};

impl<'de> toml_span::Deserialize<'de> for CondaPypiMap {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::Boolean(false) => Ok(CondaPypiMap::Disabled),
            ValueInner::Boolean(true) => Err(custom_error(
                "`conda-pypi-map = true` is not supported; use `false` to disable the \
                 mapping, or a table to configure it",
                value.span,
            )
            .into()),
            inner @ ValueInner::Table(_) => {
                let span = value.span;
                let map = TomlHashMap::<NamedChannelOrUrl, CondaPypiMapEntry>::deserialize(
                    &mut Value::with_span(inner, span),
                )?;
                Ok(CondaPypiMap::Map(map.into_inner()))
            }
            other => Err(expected("a table or `false`", other, value.span).into()),
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for CondaPypiMapEntry {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(s) => Ok(CondaPypiMapEntry::from_location(s.into_owned())),
            ValueInner::Boolean(false) => Ok(CondaPypiMapEntry::Disabled),
            ValueInner::Boolean(true) => Err(custom_error(
                "`true` is not supported; use `false` to disable the mapping for this \
                 channel, or a string or table to configure it",
                value.span,
            )
            .into()),
            inner @ ValueInner::Table(_) => {
                let table_span = value.span;
                let mut th = TableHelper::new(&mut Value::with_span(inner, table_span))?;

                let location: Option<String> = th.optional("location");
                let mapping: Option<HashMap<String, Option<String>>> = th
                    .optional::<TomlHashMap<String, TomlCondaPypiMapValue>>("mapping")
                    .map(|map| {
                        map.into_inner()
                            .into_iter()
                            .map(|(name, value)| (name, value.0))
                            .collect()
                    });
                let mode = th
                    .optional::<TomlEnum<CondaPypiMapMode>>("mode")
                    .map(TomlEnum::into_inner)
                    .unwrap_or_default();
                let cache_ttl = match th.optional::<toml_span::Spanned<String>>("cache-ttl") {
                    Some(spanned) => Some(
                        spanned
                            .value
                            .parse::<humantime::Duration>()
                            .map_err(|e| {
                                custom_error(
                                    format!("invalid `cache-ttl` duration: {e}"),
                                    spanned.span,
                                )
                            })?
                            .into(),
                    ),
                    None => None,
                };

                th.finalize(None)?;

                if location.is_none() && mapping.is_none() {
                    return Err(custom_error(
                        "expected at least one of `location` or `mapping`",
                        table_span,
                    )
                    .into());
                }

                // `cache-ttl` is part of the location source; without a
                // location it has nothing to apply to.
                let location = match (location, cache_ttl) {
                    (Some(location), cache_ttl) => Some(MappingLocationSpec {
                        location,
                        cache_ttl,
                    }),
                    (None, Some(_)) => {
                        return Err(custom_error(
                            "`cache-ttl` requires a `location` that is a URL",
                            table_span,
                        )
                        .into());
                    }
                    (None, None) => None,
                };

                Ok(CondaPypiMapEntry::Map(CondaPypiMapSpec {
                    location,
                    mapping,
                    mode,
                }))
            }
            other => Err(expected("a string, table or `false`", other, value.span).into()),
        }
    }
}

/// The value of an inline mapping entry: a pypi name, or `false` to mark the
/// package as not available on PyPI.
pub(crate) struct TomlCondaPypiMapValue(pub(crate) Option<String>);

impl<'de> toml_span::Deserialize<'de> for TomlCondaPypiMapValue {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(s) => Ok(Self(Some(s.into_owned()))),
            ValueInner::Boolean(false) => Ok(Self(None)),
            ValueInner::Boolean(true) => Err(custom_error(
                "`true` is not supported; use a string to map the package to a PyPI name, \
                 or `false` to mark it as not a PyPI package",
                value.span,
            )
            .into()),
            other => Err(expected("a string or `false`", other, value.span).into()),
        }
    }
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use insta::assert_snapshot;
    use rattler_conda_types::NamedChannelOrUrl;

    use super::*;
    use crate::{
        toml::{FromTomlStr, TomlWorkspace},
        utils::test_utils::{expect_parse_failure, expect_parse_warnings},
    };

    fn parse_map(conda_pypi_map: &str) -> CondaPypiMap {
        let input = format!(
            r#"
            channels = []
            platforms = []
            conda-pypi-map = {conda_pypi_map}
            "#
        );
        TomlWorkspace::from_toml_str(&input)
            .expect("parsing should succeed")
            .conda_pypi_map
            .expect("conda-pypi-map should be set")
    }

    fn get_entry(map: &CondaPypiMap, channel: &str) -> CondaPypiMapEntry {
        let CondaPypiMap::Map(map) = map else {
            panic!("expected a per-channel map");
        };
        map.get(&NamedChannelOrUrl::Name(channel.to_string()))
            .expect("channel should be present")
            .clone()
    }

    #[test]
    fn test_bare_string_is_extend() {
        let map = parse_map(r#"{ conda-forge = "mapping.json" }"#);
        assert_eq!(
            get_entry(&map, "conda-forge"),
            CondaPypiMapEntry::Map(CondaPypiMapSpec {
                location: Some(MappingLocationSpec {
                    location: "mapping.json".to_string(),
                    cache_ttl: None,
                }),
                mapping: None,
                mode: CondaPypiMapMode::Extend,
            })
        );
    }

    #[test]
    fn test_table_with_location_mode_and_ttl() {
        let map = parse_map(
            r#"{ conda-forge = { location = "https://example.com/m.json", mode = "replace", cache-ttl = "24h" } }"#,
        );
        assert_eq!(
            get_entry(&map, "conda-forge"),
            CondaPypiMapEntry::Map(CondaPypiMapSpec {
                location: Some(MappingLocationSpec {
                    location: "https://example.com/m.json".to_string(),
                    cache_ttl: Some(Duration::from_secs(24 * 60 * 60)),
                }),
                mapping: None,
                mode: CondaPypiMapMode::Replace,
            })
        );
    }

    #[test]
    fn test_inline_mapping_with_false_value() {
        let map = parse_map(
            r#"{ conda-forge = { mapping = { pytorch = "torch", not-on-pypi = false } } }"#,
        );
        let CondaPypiMapEntry::Map(CondaPypiMapSpec { mapping, mode, .. }) =
            get_entry(&map, "conda-forge")
        else {
            panic!("expected a mapping entry");
        };
        let mapping = mapping.expect("mapping should be set");
        assert_eq!(mode, CondaPypiMapMode::Extend);
        assert_eq!(mapping["pytorch"], Some("torch".to_string()));
        assert_eq!(mapping["not-on-pypi"], None);
    }

    #[test]
    fn test_channel_false_disables() {
        let map = parse_map(r#"{ conda-forge = false }"#);
        assert_eq!(get_entry(&map, "conda-forge"), CondaPypiMapEntry::Disabled);
    }

    #[test]
    fn test_top_level_false_disables() {
        let map = parse_map("false");
        assert_eq!(map, CondaPypiMap::Disabled);
    }

    #[test]
    fn test_empty_map_parses_and_warns() {
        let map = parse_map("{}");
        assert!(matches!(map, CondaPypiMap::Map(map) if map.is_empty()));

        assert_snapshot!(expect_parse_warnings(
            r#"
            [workspace]
            channels = []
            platforms = []
            conda-pypi-map = {}
            "#
        ));
    }

    #[test]
    fn test_top_level_true_fails() {
        assert_snapshot!(expect_parse_failure(
            r#"
            [workspace]
            channels = []
            platforms = []
            conda-pypi-map = true
            "#
        ));
    }

    #[test]
    fn test_channel_true_fails() {
        assert_snapshot!(expect_parse_failure(
            r#"
            [workspace]
            channels = []
            platforms = []
            conda-pypi-map = { conda-forge = true }
            "#
        ));
    }

    #[test]
    fn test_inline_true_value_fails() {
        assert_snapshot!(expect_parse_failure(
            r#"
            [workspace]
            channels = []
            platforms = []
            conda-pypi-map = { conda-forge = { mapping = { pytorch = true } } }
            "#
        ));
    }

    #[test]
    fn test_empty_entry_table_fails() {
        assert_snapshot!(expect_parse_failure(
            r#"
            [workspace]
            channels = []
            platforms = []
            conda-pypi-map = { conda-forge = { mode = "extend" } }
            "#
        ));
    }

    #[test]
    fn test_bogus_mode_fails() {
        assert_snapshot!(expect_parse_failure(
            r#"
            [workspace]
            channels = []
            platforms = []
            conda-pypi-map = { conda-forge = { location = "m.json", mode = "bogus" } }
            "#
        ));
    }

    #[test]
    fn test_invalid_ttl_fails() {
        assert_snapshot!(expect_parse_failure(
            r#"
            [workspace]
            channels = []
            platforms = []
            conda-pypi-map = { conda-forge = { location = "https://example.com/m.json", cache-ttl = "bogus" } }
            "#
        ));
    }

    #[test]
    fn test_ttl_without_location_fails() {
        assert_snapshot!(expect_parse_failure(
            r#"
            [workspace]
            channels = []
            platforms = []
            conda-pypi-map = { conda-forge = { mapping = { a = "b" }, cache-ttl = "24h" } }
            "#
        ));
    }
}
