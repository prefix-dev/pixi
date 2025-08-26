use std::{collections::BTreeMap, sync::Once};

use indexmap::IndexMap;
use pixi_spec::{SourceLocationSpec, TomlLocationSpec, TomlSpec};
use pixi_toml::{Same, TomlFromStr, TomlIndexMap, TomlWith};
use rattler_conda_types::NamedChannelOrUrl;
use std::borrow::Cow;
use toml_span::{DeserError, Error, Spanned, Value, de_helpers::TableHelper, value::ValueInner};

use crate::{
    PackageBuild, TargetSelector, TomlError, WithWarnings,
    build_system::BuildBackend,
    error::GenericError,
    toml::build_target::TomlPackageBuildTarget,
    utils::{PixiSpanned, package_map::UniquePackageMap},
    warning::Deprecation,
};

#[derive(Debug)]
pub struct TomlPackageBuild {
    pub backend: PixiSpanned<TomlBuildBackend>,
    pub channels: Option<PixiSpanned<Vec<NamedChannelOrUrl>>>,
    pub additional_dependencies: UniquePackageMap,
    pub source: Option<SourceLocationSpec>,
    pub configuration: Option<serde_value::Value>,
    pub target: IndexMap<PixiSpanned<TargetSelector>, TomlPackageBuildTarget>,
    pub warnings: Vec<crate::Warning>,
}

#[derive(Debug)]
pub struct TomlBuildBackend {
    pub name: PixiSpanned<rattler_conda_types::PackageName>,
    pub spec: TomlSpec,
    pub channels: Option<PixiSpanned<Vec<NamedChannelOrUrl>>>,
    pub additional_dependencies: UniquePackageMap,
}

impl TomlPackageBuild {
    pub fn into_build_system(self) -> Result<WithWarnings<PackageBuild>, TomlError> {
        // Parse the build backend and ensure it is a binary spec.
        let build_backend_spec = self.backend.value.spec.into_spec().map_err(|e| {
            TomlError::Generic(
                GenericError::new(e.to_string()).with_opt_span(self.backend.span.clone()),
            )
        })?;

        // Convert the additional dependencies and make sure that they are binary.
        // Prioritize backend.additional_dependencies over top-level additional_dependencies
        let additional_dependencies =
            if !self.backend.value.additional_dependencies.specs.is_empty() {
                self.backend.value.additional_dependencies.specs
            } else if !self.additional_dependencies.specs.is_empty() {
                self.additional_dependencies.specs
            } else {
                Default::default()
            };

        // Determine the channels to use, prioritizing backend.channels over top-level channels
        let channels = if let Some(backend_channels) = &self.backend.value.channels {
            if backend_channels.value.is_empty() {
                return Err(TomlError::Generic(
                    GenericError::new("No channels specified for the build backend dependencies")
                        .with_opt_span(backend_channels.span()),
                ));
            }
            Some(backend_channels.value.clone())
        } else if let Some(legacy_channels) = &self.channels {
            // Legacy top-level channels which are deprecated for migration purposes
            if legacy_channels.value.is_empty() {
                return Err(TomlError::Generic(
                    GenericError::new("No channels specified for the build backend dependencies")
                        .with_opt_span(legacy_channels.span()),
                ));
            }
            Some(legacy_channels.value.clone())
        } else {
            None
        };

        // Convert target-specific build config
        let target_config = self
            .target
            .into_iter()
            .flat_map(|(selector, target)| {
                target.config.map(|config| (selector.into_inner(), config))
            })
            .collect::<IndexMap<_, _>>();

        Ok(WithWarnings {
            value: PackageBuild {
                backend: BuildBackend {
                    name: self.backend.value.name.value,
                    spec: build_backend_spec,
                },
                additional_dependencies,
                channels,
                source: self.source,
                config: self.configuration,
                target_config: if target_config.is_empty() {
                    None
                } else {
                    Some(target_config)
                },
            },
            warnings: self.warnings,
        })
    }
}

pub fn convert_toml_to_serde(value: &mut Value) -> Result<serde_value::Value, DeserError> {
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
        let channels = th
            .optional_s::<TomlWith<_, Vec<TomlFromStr<_>>>>("channels")
            .map(|s| PixiSpanned {
                value: s.value.into_inner(),
                span: Some(s.span.start..s.span.end),
            });
        let additional_dependencies: UniquePackageMap =
            th.optional("additional-dependencies").unwrap_or_default();
        th.finalize(Some(value))?;

        let spec = toml_span::Deserialize::deserialize(value)?;

        Ok(TomlBuildBackend {
            name: PixiSpanned::from(Spanned {
                value: name.value.into_inner(),
                span: name.span,
            }),
            spec,
            channels,
            additional_dependencies,
        })
    }
}

static BUILD_CHANNELS_DEPRECATION: Once = Once::new();
static BOTH_CHANNELS_WARNING: Once = Once::new();
static BUILD_ADDITIONAL_DEPS_DEPRECATION: Once = Once::new();
static BOTH_ADDITIONAL_DEPS_WARNING: Once = Once::new();

fn spec_from_spanned_toml_location(
    spanned_toml: Spanned<TomlLocationSpec>,
) -> Result<SourceLocationSpec, DeserError> {
    spanned_toml
        .value
        .into_source_location_spec()
        .map_err(|err| {
            DeserError::from(Error {
                kind: toml_span::ErrorKind::Custom(Cow::Owned(err.to_string())),
                span: spanned_toml.span,
                line_info: None,
            })
        })
}

impl<'de> toml_span::Deserialize<'de> for TomlPackageBuild {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let mut warnings = Vec::new();

        let build_backend: PixiSpanned<TomlBuildBackend> = th.required_s("backend")?.into();
        let channels = th
            .optional_s::<TomlWith<_, Vec<TomlFromStr<_>>>>("channels")
            .map(|s| PixiSpanned {
                value: s.value.into_inner(),
                span: Some(s.span.start..s.span.end),
            });
        let additional_dependencies: UniquePackageMap =
            th.optional("additional-dependencies").unwrap_or_default();

        let source = th
            .optional_s::<TomlLocationSpec>("source")
            .map(spec_from_spanned_toml_location)
            .transpose()?;

        // Try the new "config" key first, then fall back to deprecated "configuration"
        let configuration = if let Some((_, mut value)) = th.take("config") {
            Some(convert_toml_to_serde(&mut value)?)
        } else if let Some((key, mut value)) = th.table.remove_entry("configuration") {
            warnings.push(Deprecation::renamed_field("configuration", "config", key.span).into());
            Some(convert_toml_to_serde(&mut value)?)
        } else {
            None
        };

        let target = th
            .optional::<TomlWith<_, TomlIndexMap<_, Same>>>("target")
            .map(TomlWith::into_inner)
            .unwrap_or_default();

        th.finalize(None)?;

        // Issue a warning if both legacy channels and backend.channels are present
        if let (Some(_), Some(_)) = (&channels, &build_backend.value.channels) {
            BOTH_CHANNELS_WARNING.call_once(|| {
                eprintln!("{}Warning: Both top-level 'channels' and 'backend.channels' are specified. Using 'backend.channels'.",
                    console::style(console::Emoji("⚠️ ", "")).yellow(),
                );
            });
        }

        // Issue a migration warning if legacy channels are used
        if channels.is_some() && build_backend.value.channels.is_none() {
            BUILD_CHANNELS_DEPRECATION.call_once(|| {
                eprintln!("{}Warning: Top-level 'channels' in [package.build] is deprecated. Please move to 'backend.channels'.",
                    console::style(console::Emoji("⚠️ ", "")).yellow(),
                );
            });
        }

        // Issue a warning if both legacy additional-dependencies and backend.additional-dependencies are present
        if !additional_dependencies.specs.is_empty()
            && !build_backend.value.additional_dependencies.specs.is_empty()
        {
            BOTH_ADDITIONAL_DEPS_WARNING.call_once(|| {
                eprintln!("{}Warning: Both top-level 'additional-dependencies' and 'backend.additional-dependencies' are specified. Using 'backend.additional-dependencies'.",
                    console::style(console::Emoji("⚠️ ", "")).yellow(),
                );
            });
        }

        // Issue a migration warning if legacy additional-dependencies are used
        if !additional_dependencies.specs.is_empty()
            && build_backend.value.additional_dependencies.specs.is_empty()
        {
            BUILD_ADDITIONAL_DEPS_DEPRECATION.call_once(|| {
                eprintln!("{}Warning: Top-level 'additional-dependencies' in [package.build] is deprecated. Please move to 'backend.additional-dependencies'.",
                    console::style(console::Emoji("⚠️ ", "")).yellow(),
                );
            });
        }

        Ok(Self {
            backend: build_backend,
            channels,
            additional_dependencies,
            source,
            configuration,
            target,
            warnings,
        })
    }
}

#[cfg(test)]
mod test {
    use insta::assert_snapshot;
    use pixi_test_utils::format_parse_error;

    use super::*;

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
    fn test_config_parsing() {
        let toml = r#"
            backend = { name = "foobar", version = "*" }
            config = { key = "value", other = ["foo", "bar"], integer = 1234, nested = { abc = "def" } }
        "#;
        let parsed = <TomlPackageBuild as crate::toml::FromTomlStr>::from_toml_str(toml)
            .and_then(TomlPackageBuild::into_build_system)
            .expect("parsing should succeed");

        assert!(parsed.warnings.is_empty());
        insta::assert_debug_snapshot!(parsed.value);
    }

    #[test]
    fn test_configuration_deprecation_warning() {
        let toml = r#"
            backend = { name = "foobar", version = "*" }
            configuration = { key = "value" }
        "#;
        let parsed = <TomlPackageBuild as crate::toml::FromTomlStr>::from_toml_str(toml)
            .and_then(TomlPackageBuild::into_build_system)
            .expect("parsing should succeed");

        assert_eq!(parsed.warnings.len(), 1);
        insta::assert_snapshot!(format_parse_error(
            toml,
            parsed.warnings.into_iter().next().unwrap()
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
    fn test_empty_channels_legacy() {
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

    #[test]
    fn test_backend_channels_new_format() {
        let toml = r#"
            backend = { name = "foobar", version = "*", channels = ["https://prefix.dev/conda-forge"] }
        "#;
        let parsed = <TomlPackageBuild as crate::toml::FromTomlStr>::from_toml_str(toml)
            .and_then(TomlPackageBuild::into_build_system)
            .expect("parsing should succeed");

        assert_eq!(parsed.value.channels.unwrap().len(), 1);
    }

    #[test]
    fn test_backend_channels_legacy_format() {
        let toml = r#"
            backend = { name = "foobar", version = "*" }
            channels = ["https://prefix.dev/conda-forge"]
        "#;
        let parsed = <TomlPackageBuild as crate::toml::FromTomlStr>::from_toml_str(toml)
            .and_then(TomlPackageBuild::into_build_system)
            .expect("parsing should succeed");

        assert_eq!(parsed.value.channels.unwrap().len(), 1);
    }

    #[test]
    fn test_backend_channels_priority() {
        let toml = r#"
            backend = { name = "foobar", version = "*", channels = ["https://prefix.dev/pixi-build-backends"] }
            channels = ["https://prefix.dev/conda-forge"]
        "#;
        let parsed = <TomlPackageBuild as crate::toml::FromTomlStr>::from_toml_str(toml)
            .and_then(TomlPackageBuild::into_build_system)
            .expect("parsing should succeed");

        // Should use backend.channels, not top-level channels
        let channels = parsed.value.channels.unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(
            channels[0].to_string(),
            "https://prefix.dev/pixi-build-backends"
        );
    }

    #[test]
    fn test_empty_backend_channels() {
        assert_snapshot!(expect_parse_failure(
            r#"
            backend = { name = "foobar", version = "*", channels = [] }
        "#
        ));
    }

    #[test]
    fn test_backend_additional_dependencies() {
        let toml = r#"
            backend = { name = "foobar", version = "*", additional-dependencies = { git = "*" } }
        "#;
        let parsed = <TomlPackageBuild as crate::toml::FromTomlStr>::from_toml_str(toml)
            .and_then(TomlPackageBuild::into_build_system)
            .expect("parsing should succeed");

        assert!(!parsed.value.additional_dependencies.is_empty());
        assert!(
            parsed
                .value
                .additional_dependencies
                .contains_key(&"git".parse::<rattler_conda_types::PackageName>().unwrap())
        );
    }

    #[test]
    fn test_legacy_additional_dependencies() {
        let toml = r#"
            backend = { name = "foobar", version = "*" }
            additional-dependencies = { git = "*" }
        "#;
        let parsed = <TomlPackageBuild as crate::toml::FromTomlStr>::from_toml_str(toml)
            .and_then(TomlPackageBuild::into_build_system)
            .expect("parsing should succeed");

        assert!(!parsed.value.additional_dependencies.is_empty());
        assert!(
            parsed
                .value
                .additional_dependencies
                .contains_key(&"git".parse::<rattler_conda_types::PackageName>().unwrap())
        );
    }

    #[test]
    fn test_backend_additional_dependencies_priority() {
        let toml = r#"
            backend = { name = "foobar", version = "*", additional-dependencies = { rust = "*" } }
            additional-dependencies = { git = "*" }
        "#;
        let parsed = <TomlPackageBuild as crate::toml::FromTomlStr>::from_toml_str(toml)
            .and_then(TomlPackageBuild::into_build_system)
            .expect("parsing should succeed");

        // Should prioritize backend.additional-dependencies
        assert!(!parsed.value.additional_dependencies.is_empty());
        assert!(
            parsed
                .value
                .additional_dependencies
                .contains_key(&"rust".parse::<rattler_conda_types::PackageName>().unwrap())
        );
        assert!(
            !parsed
                .value
                .additional_dependencies
                .contains_key(&"git".parse::<rattler_conda_types::PackageName>().unwrap())
        );
    }
}
