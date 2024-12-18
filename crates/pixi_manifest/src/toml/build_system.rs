use itertools::Either;
use pixi_spec::TomlSpec;
use pixi_toml::{TomlFromStr, TomlWith};
use rattler_conda_types::NamedChannelOrUrl;
use toml_span::{de_helpers::TableHelper, DeserError, Spanned, Value};

use crate::{
    build_system::BuildBackend,
    utils::{package_map::UniquePackageMap, PixiSpanned},
    PackageBuild, TomlError,
};

#[derive(Debug)]
pub struct TomlPackageBuild {
    pub backend: PixiSpanned<TomlBuildBackend>,
    pub channels: Option<PixiSpanned<Vec<NamedChannelOrUrl>>>,
    pub additional_dependencies: UniquePackageMap,
}

#[derive(Debug)]
pub struct TomlBuildBackend {
    pub name: PixiSpanned<rattler_conda_types::PackageName>,
    pub spec: TomlSpec,
}

impl TomlPackageBuild {
    pub fn into_build_system(self) -> Result<PackageBuild, TomlError> {
        // Parse the build backend and ensure it is a binary spec.
        let build_backend_spec = self
            .backend
            .value
            .spec
            .into_binary_spec()
            .map_err(|e| {
                TomlError::Generic(e.to_string().into(), self.backend.span.clone())
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
                        "Cannot use source dependencies for build backends dependencies".into(),
                        spec_range,
                    ))
                }
            })
            .collect::<Result<_, TomlError>>()?;

        // Make sure there are no empty channels
        if let Some(channels) = &self.channels {
            if channels.value.is_empty() {
                return Err(TomlError::Generic(
                    "No channels specified for the build backend dependencies".into(),
                    channels.span(),
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
        })
    }
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
        th.finalize(None)?;
        Ok(Self {
            backend: build_backend,
            channels,
            additional_dependencies,
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
