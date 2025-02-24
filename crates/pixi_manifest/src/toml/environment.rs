use toml_span::{de_helpers::expected, DeserError, Spanned, Value};

/// Helper struct to deserialize the environment from TOML.
/// The environment description can only hold these values.
#[derive(Debug)]
pub struct TomlEnvironment {
    pub features: Option<Spanned<Vec<Spanned<String>>>>,
    pub solve_group: Option<String>,
    pub no_default_feature: bool,
}

#[derive(Debug)]
pub enum TomlEnvironmentList {
    Map(TomlEnvironment),
    Seq(Spanned<Vec<Spanned<String>>>),
}

impl<'de> toml_span::Deserialize<'de> for TomlEnvironment {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = toml_span::de_helpers::TableHelper::new(value)?;

        let features = th.optional_s("features");
        let solve_group = th.optional("solve-group");
        let no_default_feature = th.optional("no-default-feature");

        th.finalize(None)?;

        if features.is_none() && solve_group.is_none() {
            return Err(DeserError::from(toml_span::Error {
                kind: toml_span::ErrorKind::MissingField("features"),
                span: value.span,
                line_info: None,
            }));
        }

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
    use crate::{toml::FromTomlStr, utils::test_utils::format_parse_error};

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
            TomlEnvironmentList::Seq(envs) if envs.value.clone().into_iter().map(Spanned::take).collect::<Vec<_>>() == vec!["foo", "bar"]);
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
                map.features.clone().unwrap().value.into_iter().map(Spanned::take).collect::<Vec<_>>() == vec!["foo", "bar"]
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
    pub fn test_parse_empty_environment() {
        let input = r#"
            env = {}
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

    #[test]
    pub fn test_parse_features_is_optional() {
        let input = r#"
            env = { solve-group = "group" }
        "#;

        let top_level = TopLevel::from_toml_str(input).unwrap();
        assert_matches!(top_level.env, TomlEnvironmentList::Map(_));
    }
}
