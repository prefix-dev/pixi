use serde::{Deserialize, Deserializer};
use toml_span::{de_helpers::expected, DeserError, Value};

use crate::utils::PixiSpanned;

/// Helper struct to deserialize the environment from TOML.
/// The environment description can only hold these values.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TomlEnvironment {
    #[serde(default)]
    pub features: PixiSpanned<Vec<String>>,
    pub solve_group: Option<String>,
    #[serde(default)]
    pub no_default_feature: bool,
}

#[derive(Debug)]
pub enum TomlEnvironmentList {
    Map(TomlEnvironment),
    Seq(Vec<String>),
}

impl<'de> Deserialize<'de> for TomlEnvironmentList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .map(|map| map.deserialize().map(TomlEnvironmentList::Map))
            .seq(|seq| seq.deserialize().map(TomlEnvironmentList::Seq))
            .expecting("either a map or a sequence")
            .deserialize(deserializer)
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlEnvironment {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = toml_span::de_helpers::TableHelper::new(value)?;

        let features = th.required_s("features")?.into();
        let solve_group = th.optional("solve-group");
        let no_default_feature = th.optional("no-default-feature");

        th.finalize(None)?;

        Ok(TomlEnvironment {
            features,
            solve_group,
            no_default_feature: no_default_feature.unwrap_or_default(),
        })
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlEnvironmentList {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        if value.as_array().is_some() {
            Ok(TomlEnvironmentList::Seq(
                toml_span::Deserialize::deserialize(value)?,
            ))
        } else if value.as_table().is_some() {
            Ok(TomlEnvironmentList::Map(
                toml_span::Deserialize::deserialize(value)?,
            ))
        } else {
            Err(expected("either a map or a sequence", value.take(), value.span).into())
        }
    }
}

#[cfg(test)]
mod test {
    use assert_matches::assert_matches;
    use insta::assert_snapshot;
    use toml_span::{de_helpers::TableHelper, DeserError, Value};

    use super::*;
    use crate::toml::FromTomlStr;
    use crate::utils::test_utils::format_parse_error;

    #[derive(Debug)]
    struct TopLevel {
        env: TomlEnvironmentList,
    }

    impl<'de> toml_span::Deserialize<'de> for TopLevel {
        fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
            let mut th = TableHelper::new(value)?;
            let env = th.required("env")?;
            th.finalize(None)?;
            Ok(TopLevel { env })
        }
    }

    #[test]
    pub fn test_parse_environment() {
        let input = r#"
            env = ["foo", "bar"]
        "#;

        let toplevel = TopLevel::from_toml_str(input).unwrap();
        assert_matches!(
            toplevel.env,
            TomlEnvironmentList::Seq(envs) if envs == vec!["foo", "bar"]);
    }

    #[test]
    pub fn test_parse_map_environment() {
        let input = r#"
            env = { features = ["foo", "bar"], solve-group = "group", no-default-feature = true }
        "#;

        let toplevel = TopLevel::from_toml_str(input).unwrap();
        assert_matches!(
            toplevel.env,
            TomlEnvironmentList::Map(map) if
                map.features.value == vec!["foo", "bar"]
                && map.solve_group == Some("group".to_string())
                && map.no_default_feature);
    }

    #[test]
    pub fn test_parse_invalid_environment() {
        let input = r#"
            env = { feat = ["foo", "bar"] }
        "#;

        assert_snapshot!(format_parse_error(
            input,
            TopLevel::from_toml_str(input).unwrap_err()
        ));
    }

    #[test]
    pub fn test_parse_invalid_environment_feature_type() {
        let input = r#"
            env = { features = [123] }
        "#;

        assert_snapshot!(format_parse_error(
            input,
            TopLevel::from_toml_str(input).unwrap_err()
        ));
    }

    #[test]
    pub fn test_parse_invalid_solve_group() {
        let input = r#"
            env = { features = [], solve_groups = "group" }
        "#;

        assert_snapshot!(format_parse_error(
            input,
            TopLevel::from_toml_str(input).unwrap_err()
        ));
    }
}
