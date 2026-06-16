//! TOML deserialization for `[workspace.conda-pypi-map]`.
//!
//! The field accepts `false` (disable all lookups) or a per-channel table.
//! Each channel value is either a bare location string, `false` (disable
//! lookups for that channel) or a table with `location`, inline `mapping`
//! entries, `mapping-mode` and `same-name-heuristic`.

use std::collections::HashMap;

use pixi_toml::{TomlEnum, TomlHashMap, custom_error};
use rattler_conda_types::NamedChannelOrUrl;
use toml_span::{
    DeserError, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

use crate::workspace::{
    CondaPypiMap, CondaPypiMapEntry, CondaPypiMapSpec, CondaPypiMappingMode,
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
                let mapping: Option<HashMap<String, Vec<String>>> = th
                    .optional::<TomlHashMap<String, TomlCondaPypiMapValue>>("mapping")
                    .map(|map| {
                        map.into_inner()
                            .into_iter()
                            .map(|(name, value)| (name, value.0))
                            .collect()
                    });
                let mapping_mode = th
                    .optional::<TomlEnum<CondaPypiMappingMode>>("mapping-mode")
                    .map(TomlEnum::into_inner);
                let same_name_heuristic = th.optional::<bool>("same-name-heuristic");

                th.finalize(None)?;

                if location.is_none()
                    && mapping.is_none()
                    && same_name_heuristic.is_none()
                    && mapping_mode.is_none()
                {
                    return Err(custom_error(
                        "expected at least one of `location`, `mapping`, `mapping-mode` or `same-name-heuristic`",
                        table_span,
                    )
                    .into());
                }

                Ok(CondaPypiMapEntry::Map(CondaPypiMapSpec {
                    location,
                    mapping,
                    mapping_mode: mapping_mode.unwrap_or_default(),
                    same_name_heuristic,
                }))
            }
            other => Err(expected("a string, table or `false`", other, value.span).into()),
        }
    }
}

/// The value of an inline mapping entry: a pypi name, a list of pypi names,
/// or `false` to mark the package as not available on PyPI (normalized to an
/// empty list).
pub(crate) struct TomlCondaPypiMapValue(pub(crate) Vec<String>);

impl<'de> toml_span::Deserialize<'de> for TomlCondaPypiMapValue {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(s) => Ok(Self(vec![s.into_owned()])),
            ValueInner::Array(items) => {
                let mut names = Vec::with_capacity(items.len());
                for mut item in items {
                    match item.take() {
                        ValueInner::String(s) => names.push(s.into_owned()),
                        other => return Err(expected("a string", other, item.span).into()),
                    }
                }
                Ok(Self(names))
            }
            ValueInner::Boolean(false) => Ok(Self(Vec::new())),
            ValueInner::Boolean(true) => Err(custom_error(
                "`true` is not supported; use a string or a list of strings to map the \
                 package to PyPI name(s), or `false` to mark it as not a PyPI package",
                value.span,
            )
            .into()),
            other => {
                Err(expected("a string, a list of strings or `false`", other, value.span).into())
            }
        }
    }
}

#[cfg(test)]
mod test {
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
    fn test_bare_string_is_overlay() {
        let map = parse_map(r#"{ conda-forge = "mapping.json" }"#);
        assert_eq!(
            get_entry(&map, "conda-forge"),
            CondaPypiMapEntry::Map(CondaPypiMapSpec {
                location: Some("mapping.json".to_string()),
                mapping: None,
                mapping_mode: CondaPypiMappingMode::Overlay,
                same_name_heuristic: None,
            })
        );
    }

    #[test]
    fn test_table_with_location_and_mapping_mode() {
        let map = parse_map(
            r#"{ conda-forge = { location = "https://example.com/m.json", mapping-mode = "replace" } }"#,
        );
        assert_eq!(
            get_entry(&map, "conda-forge"),
            CondaPypiMapEntry::Map(CondaPypiMapSpec {
                location: Some("https://example.com/m.json".to_string()),
                mapping: None,
                mapping_mode: CondaPypiMappingMode::Replace,
                same_name_heuristic: None,
            })
        );
    }

    #[test]
    fn test_inline_mapping_with_false_value() {
        let map = parse_map(
            r#"{ conda-forge = { mapping = { pytorch = "torch", not-on-pypi = false } } }"#,
        );
        let CondaPypiMapEntry::Map(CondaPypiMapSpec {
            mapping,
            mapping_mode,
            ..
        }) = get_entry(&map, "conda-forge")
        else {
            panic!("expected a mapping entry");
        };
        let mapping = mapping.expect("mapping should be set");
        assert_eq!(mapping_mode, CondaPypiMappingMode::Overlay);
        assert_eq!(mapping["pytorch"], vec!["torch".to_string()]);
        assert_eq!(mapping["not-on-pypi"], Vec::<String>::new());
    }

    #[test]
    fn test_inline_mapping_with_list_value() {
        let map = parse_map(
            r#"{ conda-forge = { mapping = { airflow = ["airflow", "apache-airflow"] } } }"#,
        );
        let CondaPypiMapEntry::Map(CondaPypiMapSpec { mapping, .. }) =
            get_entry(&map, "conda-forge")
        else {
            panic!("expected a mapping entry");
        };
        let mapping = mapping.expect("mapping should be set");
        assert_eq!(
            mapping["airflow"],
            vec!["airflow".to_string(), "apache-airflow".to_string()]
        );
    }

    #[test]
    fn test_inline_mapping_empty_list_means_not_on_pypi() {
        let map = parse_map(r#"{ conda-forge = { mapping = { not-on-pypi = [] } } }"#);
        let CondaPypiMapEntry::Map(CondaPypiMapSpec { mapping, .. }) =
            get_entry(&map, "conda-forge")
        else {
            panic!("expected a mapping entry");
        };
        assert_eq!(
            mapping.expect("mapping should be set")["not-on-pypi"],
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_inline_list_with_non_string_fails() {
        assert_snapshot!(expect_parse_failure(
            r#"
            [workspace]
            channels = []
            platforms = []
            conda-pypi-map = { conda-forge = { mapping = { pytorch = ["torch", 1] } } }
            "#
        ));
    }

    #[test]
    fn test_mapping_mode_only_entry_parses_as_empty_mapping() {
        let map = parse_map(r#"{ conda-forge = { mapping-mode = "replace" } }"#);
        assert_eq!(
            get_entry(&map, "conda-forge"),
            CondaPypiMapEntry::Map(CondaPypiMapSpec {
                location: None,
                mapping: None,
                mapping_mode: CondaPypiMappingMode::Replace,
                same_name_heuristic: None,
            })
        );
    }

    #[test]
    fn test_same_name_heuristic_only_entry_parses() {
        let map = parse_map(r#"{ conda-forge = { same-name-heuristic = false } }"#);
        assert_eq!(
            get_entry(&map, "conda-forge"),
            CondaPypiMapEntry::Map(CondaPypiMapSpec {
                location: None,
                mapping: None,
                mapping_mode: CondaPypiMappingMode::Overlay,
                same_name_heuristic: Some(false),
            })
        );
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
            conda-pypi-map = { conda-forge = {} }
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
            conda-pypi-map = { conda-forge = { location = "m.json", mapping-mode = "bogus" } }
            "#
        ));
    }

}
