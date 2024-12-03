use itertools::Either;
use pixi_spec::TomlSpec;
use rattler_conda_types::NamedChannelOrUrl;
use serde::Deserialize;

use crate::{
    build_system::BuildBackend,
    utils::{package_map::UniquePackageMap, PixiSpanned},
    BuildSystem, TomlError,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TomlBuildSystem {
    pub build_backend: PixiSpanned<TomlBuildBackend>,

    #[serde(default)]
    pub channels: Option<PixiSpanned<Vec<NamedChannelOrUrl>>>,

    #[serde(default)]
    pub additional_dependencies: UniquePackageMap,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TomlBuildBackend {
    pub name: rattler_conda_types::PackageName,

    #[serde(flatten)]
    pub spec: TomlSpec,
}

impl TomlBuildSystem {
    pub fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        toml_edit::de::from_str(source).map_err(TomlError::from)
    }

    pub fn into_build_system(self) -> Result<BuildSystem, TomlError> {
        // Parse the build backend and ensure it is a binary spec.
        let build_backend_spec = self
            .build_backend
            .value
            .spec
            .into_binary_spec()
            .map_err(|e| {
                TomlError::Generic(e.to_string().into(), self.build_backend.span.clone())
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

        Ok(BuildSystem {
            build_backend: BuildBackend {
                name: self.build_backend.value.name,
                spec: build_backend_spec,
            },
            additional_dependencies,
            channels: self.channels.map(|channels| channels.value),
        })
    }
}

#[cfg(test)]
mod test {
    use insta::assert_snapshot;

    use super::*;
    use crate::utils::test_utils::format_parse_error;

    fn expect_parse_failure(pixi_toml: &str) -> String {
        let parse_error = TomlBuildSystem::from_toml_str(pixi_toml)
            .and_then(TomlBuildSystem::into_build_system)
            .expect_err("parsing should fail");

        format_parse_error(pixi_toml, parse_error)
    }

    #[test]
    fn test_disallow_source() {
        assert_snapshot!(expect_parse_failure(
            r#"
            build-backend = { name = "foobar", git = "https://github.com/org/repo" }
        "#
        ));
    }

    #[test]
    fn test_missing_version_specifier() {
        assert_snapshot!(expect_parse_failure(
            r#"
            build-backend = { name = "foobar" }
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
            build-backend = { version = "12.*" }
        "#
        ));
    }

    #[test]
    fn test_empty_channels() {
        assert_snapshot!(expect_parse_failure(
            r#"
            build-backend = { name = "foobar", version = "*" }
            channels = []
        "#
        ));
    }

    #[test]
    fn test_additional_build_backend_keys() {
        assert_snapshot!(expect_parse_failure(
            r#"
            build-backend = { name = "foobar", version = "*", foo = "bar" }
        "#
        ));
    }

    #[test]
    fn test_additional_keys() {
        assert_snapshot!(expect_parse_failure(
            r#"
            build-backend = { name = "foobar", version = "*" }
            additional = "key"
        "#
        ));
    }
}
