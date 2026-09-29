//! Tests for `[workspace.conda-pypi-map]` parsing. The type lives in
//! `pixi_config`; parsing it as part of a manifest is tested here.

mod test {
    use insta::assert_snapshot;
    use rattler_conda_types::NamedChannelOrUrl;

    use crate::{
        CondaPypiMap, CondaPypiMapEntry, CondaPypiMapSpec, CondaPypiMappingMode,
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
