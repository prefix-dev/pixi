use std::collections::BTreeMap;

use itertools::Either;
use pixi_spec::TomlSpec;
use pixi_toml::{TomlFromStr, TomlWith};
use rattler_conda_types::NamedChannelOrUrl;
use toml_span::{de_helpers::TableHelper, value::ValueInner, DeserError, Spanned, Value};

use crate::{
    build_system::BuildBackend,
    error::GenericError,
    utils::{package_map::UniquePackageMap, PixiSpanned},
    PackageBuild, TomlError,
};

#[derive(Debug)]
pub struct TomlPackageBuild {
    pub backend: PixiSpanned<TomlBuildBackend>,
    pub channels: Option<PixiSpanned<Vec<NamedChannelOrUrl>>>,
    pub additional_dependencies: UniquePackageMap,
    pub configuration: Option<serde_value::Value>,
}

#[derive(Debug)]
pub struct TomlBuildBackend {
    pub name: PixiSpanned<rattler_conda_types::PackageName>,
    pub spec: TomlSpec,
}

impl TomlPackageBuild {
    pub fn into_build_system(self) -> Result<PackageBuild, TomlError> {
        // Parse the build backend and ensure it is a binary spec.
        let build_backend_spec = self.backend.value.spec.into_binary_spec().map_err(|e| {
            TomlError::Generic(
                GenericError::new(e.to_string()).with_opt_span(self.backend.span.clone()),
            )
        })?;

        // Convert the additional dependencies and make sure that they are binary.
        let additional_dependencies = self
            .additional_dependencies
            .specs
            .into_iter()
            .map(|(name, spec)| match spec.into_source_or_binary() {
                Either::Right(binary) => Ok((name, binary)),
                Either::Left(_source) => {
                    let spec_range = self
                        .additional_dependencies
                        .value_spans
                        .get(&name)
                        .or_else(|| self.additional_dependencies.name_spans.get(&name))
                        .cloned();
                    Err(TomlError::Generic(
                        GenericError::new(
                            "Cannot use source dependencies for build backends dependencies",
                        )
                        .with_opt_span(spec_range),
                    ))
                }
            })
            .collect::<Result<_, TomlError>>()?;

        // Make sure there are no empty channels
        if let Some(channels) = &self.channels {
            if channels.value.is_empty() {
                return Err(TomlError::Generic(
                    GenericError::new("No channels specified for the build backend dependencies")
                        .with_opt_span(channels.span()),
                ));
            }
        }

        Ok(PackageBuild {
            backend: BuildBackend {
                name: self.backend.value.name.value,
                spec: build_backend_spec,
            },
            additional_dependencies,
            channels: self.channels.map(|channels| channels.value),
            configuration: self.configuration,
        })
    }
}

fn convert_toml_to_serde(value: &mut Value) -> Result<serde_value::Value, DeserError> {
    Ok(match value.take() {
        ValueInner::String(s) => serde_value::Value::String(s.to_string()),
        ValueInner::Integer(i) => serde_value::Value::I64(i),
        ValueInner::Float(f) => serde_value::Value::F64(f),
        ValueInner::Boolean(b) => serde_value::Value::Bool(b),
        ValueInner::Array(mut arr) => {
            let mut json_arr = Vec::new();
            for item in &mut arr {
                json_arr.push(convert_toml_to_serde(item)?);
            }
            serde_value::Value::Seq(json_arr)
        }
        ValueInner::Table(table) => {
            let mut map = BTreeMap::new();
            for (key, mut val) in table {
                map.insert(
                    serde_value::Value::String(key.to_string()),
                    convert_toml_to_serde(&mut val)?,
                );
            }
            serde_value::Value::Map(map)
        }
    })
}

impl<'de> toml_span::Deserialize<'de> for TomlBuildBackend {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let name = th.required_s::<TomlFromStr<rattler_conda_types::PackageName>>("name")?;
        th.finalize(Some(value))?;

        let spec = toml_span::Deserialize::deserialize(value)?;

        Ok(TomlBuildBackend {
            name: PixiSpanned::from(Spanned {
                value: name.value.into_inner(),
                span: name.span,
            }),
            spec,
        })
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlPackageBuild {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let build_backend = th.required_s("backend")?.into();
        let channels = th
            .optional_s::<TomlWith<_, Vec<TomlFromStr<_>>>>("channels")
            .map(|s| PixiSpanned {
                value: s.value.into_inner(),
                span: Some(s.span.start..s.span.end),
            });
        let additional_dependencies = th.optional("additional-dependencies").unwrap_or_default();

        let configuration = th
            .take("configuration")
            .map(|(_, mut value)| convert_toml_to_serde(&mut value))
            .transpose()?;

        th.finalize(None)?;
        Ok(Self {
            backend: build_backend,
            channels,
            additional_dependencies,
            configuration,
        })
    }
}

#[cfg(test)]
mod test {
    use insta::assert_snapshot;

    use super::*;
    use crate::utils::test_utils::format_parse_error;

    fn expect_parse_failure(pixi_toml: &str) -> String {
        let parse_error = <TomlPackageBuild as crate::toml::FromTomlStr>::from_toml_str(pixi_toml)
            .and_then(TomlPackageBuild::into_build_system)
            .expect_err("parsing should fail");

        format_parse_error(pixi_toml, parse_error)
    }

    #[test]
    fn test_configuration_parsing() {
        let toml = r#"
            backend = { name = "foobar", version = "*" }
            configuration = { key = "value", other = ["foo", "bar"], integer = 1234, nested = { abc = "def" } }
        "#;
        let parsed = <TomlPackageBuild as crate::toml::FromTomlStr>::from_toml_str(toml)
            .expect("parsing should succeed");

        insta::assert_debug_snapshot!(parsed);
    }

    #[test]
    fn test_disallow_source() {
        assert_snapshot!(expect_parse_failure(
            r#"
            backend = { name = "foobar", git = "https://github.com/org/repo" }
        "#
        ));
    }

    #[test]
    fn test_missing_version_specifier() {
        assert_snapshot!(expect_parse_failure(
            r#"
            backend = { name = "foobar" }
        "#
        ));
    }

    #[test]
    fn test_missing_backend() {
        assert_snapshot!(expect_parse_failure(""));
    }

    #[test]
    fn test_missing_name() {
        assert_snapshot!(expect_parse_failure(
            r#"
            backend = { version = "12.*" }
        "#
        ));
    }

    #[test]
    fn test_empty_channels() {
        assert_snapshot!(expect_parse_failure(
            r#"
            backend = { name = "foobar", version = "*" }
            channels = []
        "#
        ));
    }

    #[test]
    fn test_additional_build_backend_keys() {
        assert_snapshot!(expect_parse_failure(
            r#"
            backend = { name = "foobar", version = "*", sub = "bar" }
        "#
        ));
    }

    #[test]
    fn test_additional_keys() {
        assert_snapshot!(expect_parse_failure(
            r#"
            backend = { name = "foobar", version = "*" }
            additional = "key"
        "#
        ));
    }
}
