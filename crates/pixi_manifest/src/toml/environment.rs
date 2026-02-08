use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_toml::{TomlHashMap, TomlIndexMap, TomlIndexSet, TomlWith};
use rattler_conda_types::Platform;
use toml_span::{DeserError, Spanned, Value, de_helpers::expected};

use crate::{
    Activation, SystemRequirements, Task, TaskName, Warning, WithWarnings,
    pypi::pypi_options::PypiOptions,
    toml::{TargetSelector, platform::TomlPlatform, target::TomlTarget, task::TomlTask},
    utils::{PixiSpanned, package_map::UniquePackageMap},
    warning::Deprecation,
    workspace::{ChannelPriority, SolveStrategy},
};

use super::{TomlFeature, TomlPrioritizedChannel};

/// Helper struct to deserialize an environment from TOML.
///
/// Environments can be defined in two ways:
/// 1. **Traditional**: Reference existing features with optional configuration
///    ```toml
///    [environments]
///    dev = { features = ["test", "lint"], solve-group = "dev" }
///    ```
///
/// 2. **Inline configuration**: Define dependencies and tasks directly on the environment
///    ```toml
///    [environments.dev.dependencies]
///    pytest = "*"
///
///    [environments.dev.tasks]
///    test = "pytest"
///    ```
///
/// When inline configuration is detected, a synthetic feature with the same name as
/// the environment is created and prepended to the environment's feature list.
#[derive(Debug)]
pub struct TomlEnvironment {
    pub features: Option<Spanned<Vec<Spanned<String>>>>,
    pub solve_group: Option<String>,
    pub no_default_feature: bool,
    pub platforms: Option<Spanned<IndexSet<Platform>>>,
    pub channels: Option<Vec<TomlPrioritizedChannel>>,
    pub channel_priority: Option<ChannelPriority>,
    pub solve_strategy: Option<SolveStrategy>,
    pub system_requirements: SystemRequirements,
    pub dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub host_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub pypi_dependencies: Option<IndexMap<PypiPackageName, PixiPypiSpec>>,
    pub dev: Option<IndexMap<rattler_conda_types::PackageName, pixi_spec::TomlLocationSpec>>,
    pub activation: Option<Activation>,
    pub tasks: HashMap<TaskName, Task>,
    pub target: IndexMap<PixiSpanned<TargetSelector>, TomlTarget>,
    pub pypi_options: Option<PypiOptions>,
    pub warnings: Vec<Warning>,
}

impl TomlEnvironment {
    /// Returns `true` if this environment has inline feature configuration.
    ///
    /// Inline configuration includes any of: dependencies, tasks, activation,
    /// target-specific config, platforms, channels, or other feature fields
    /// defined directly on the environment.
    pub fn has_inline_config(&self) -> bool {
        self.dependencies.is_some()
            || self.host_dependencies.is_some()
            || self.build_dependencies.is_some()
            || self.pypi_dependencies.is_some()
            || self.dev.is_some()
            || !self.tasks.is_empty()
            || self.activation.is_some()
            || !self.target.is_empty()
            || self.platforms.is_some()
            || self.channels.is_some()
            || self.channel_priority.is_some()
            || self.solve_strategy.is_some()
            || self.pypi_options.is_some()
            || self.system_requirements != SystemRequirements::default()
    }

    pub fn take_warnings(&mut self) -> Vec<Warning> {
        std::mem::take(&mut self.warnings)
    }

    /// Converts inline configuration to a [`TomlFeature`] for synthetic feature creation.
    ///
    /// This is used when an environment has inline dependencies, tasks, or other
    /// feature configuration. The resulting `TomlFeature` can be converted to a
    /// proper `Feature` and added to the workspace's feature map.
    ///
    /// Returns `None` if this environment has no inline configuration.
    pub fn into_toml_feature(self) -> Option<TomlFeature> {
        if !self.has_inline_config() {
            return None;
        }

        Some(TomlFeature {
            platforms: self.platforms,
            channels: self.channels,
            channel_priority: self.channel_priority,
            system_requirements: self.system_requirements,
            target: self.target,
            solve_strategy: self.solve_strategy,
            dependencies: self.dependencies,
            host_dependencies: self.host_dependencies,
            build_dependencies: self.build_dependencies,
            pypi_dependencies: self.pypi_dependencies,
            dev: self.dev,
            activation: self.activation,
            tasks: self.tasks,
            pypi_options: self.pypi_options,
            warnings: self.warnings,
        })
    }
}

#[derive(Debug)]
pub enum TomlEnvironmentList {
    Map(Box<TomlEnvironment>),
    Seq(Spanned<Vec<Spanned<String>>>),
}

impl<'de> toml_span::Deserialize<'de> for TomlEnvironment {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = toml_span::de_helpers::TableHelper::new(value)?;
        let mut warnings = Vec::new();

        // Environment-specific fields
        let features = th.optional_s("features");
        let solve_group = th.optional("solve-group");
        let no_default_feature = th.optional("no-default-feature");

        // Feature fields (duplicated from TomlFeature because toml-span doesn't
        // support flatten/delegation). These are converted via `into_toml_feature()`.
        let platforms: Option<Spanned<IndexSet<Platform>>> = th
            .optional::<TomlWith<_, Spanned<TomlIndexSet<TomlPlatform>>>>("platforms")
            .map(TomlWith::into_inner);
        let channels = th.optional("channels");
        let channel_priority = th.optional("channel-priority");
        let solve_strategy = th.optional("solve-strategy");
        let target: IndexMap<PixiSpanned<TargetSelector>, TomlTarget> = th
            .optional::<TomlIndexMap<_, _>>("target")
            .map(TomlIndexMap::into_inner)
            .unwrap_or_default();
        let dependencies = th.optional("dependencies");

        // Handle deprecated host-dependencies
        let host_dependencies: Option<Spanned<UniquePackageMap>> = th.optional("host-dependencies");
        if let Some(host_dependencies) = &host_dependencies {
            warnings.push(
                Deprecation::renamed_field(
                    "host-dependencies",
                    "dependencies",
                    host_dependencies.span,
                )
                .into(),
            );
        }
        let host_dependencies = host_dependencies.map(From::from);

        // Handle deprecated build-dependencies
        let build_dependencies: Option<Spanned<UniquePackageMap>> =
            th.optional("build-dependencies");
        if let Some(build_dependencies) = &build_dependencies {
            warnings.push(
                Deprecation::renamed_field(
                    "build-dependencies",
                    "dependencies",
                    build_dependencies.span,
                )
                .into(),
            );
        }
        let build_dependencies = build_dependencies.map(From::from);

        let pypi_dependencies: Option<IndexMap<PypiPackageName, PixiPypiSpec>> = th
            .optional::<TomlIndexMap<_, _>>("pypi-dependencies")
            .map(TomlIndexMap::into_inner);
        let dev = th
            .optional::<TomlIndexMap<_, _>>("dev")
            .map(TomlIndexMap::into_inner);
        let activation = th.optional("activation");
        let pypi_options = th.optional("pypi-options");
        let system_requirements = th
            .optional::<SystemRequirements>("system-requirements")
            .unwrap_or_default();

        // Parse tasks with warning collection
        let tasks: HashMap<TaskName, Task> = th
            .optional::<TomlHashMap<_, TomlTask>>("tasks")
            .map(TomlHashMap::into_inner)
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| {
                let WithWarnings {
                    value: task,
                    warnings: mut task_warnings,
                } = value;
                warnings.append(&mut task_warnings);
                (key, task)
            })
            .collect();

        th.finalize(None)?;

        // Validation: if it's a map format without inline config, need features or solve-group
        let has_inline = dependencies.is_some()
            || host_dependencies.is_some()
            || build_dependencies.is_some()
            || pypi_dependencies.is_some()
            || dev.is_some()
            || !tasks.is_empty()
            || activation.is_some()
            || !target.is_empty()
            || platforms.is_some()
            || channels.is_some()
            || channel_priority.is_some()
            || solve_strategy.is_some()
            || pypi_options.is_some()
            || system_requirements != SystemRequirements::default();

        if !has_inline && features.is_none() && solve_group.is_none() {
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
            platforms,
            channels,
            channel_priority,
            solve_strategy,
            system_requirements,
            dependencies,
            host_dependencies,
            build_dependencies,
            pypi_dependencies,
            dev,
            activation,
            tasks,
            target,
            pypi_options,
            warnings,
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
            Ok(TomlEnvironmentList::Map(Box::new(
                toml_span::Deserialize::deserialize(value)?,
            )))
        } else {
            Err(expected("either a map or a sequence", value.take(), value.span).into())
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::toml::FromTomlStr;
    use assert_matches::assert_matches;
    use insta::assert_snapshot;
    use pixi_test_utils::format_parse_error;
    use toml_span::{DeserError, Value, de_helpers::TableHelper};

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

    #[test]
    pub fn test_parse_inline_dependencies() {
        let input = r#"
            env = { dependencies = { pytest = "*" } }
        "#;

        let toplevel = TopLevel::from_toml_str(input).unwrap();
        assert_matches!(toplevel.env, TomlEnvironmentList::Map(env) => {
            assert!(env.has_inline_config());
            assert!(env.dependencies.is_some());
        });
    }

    #[test]
    pub fn test_parse_inline_with_features() {
        let input = r#"
            env = { features = ["python"], dependencies = { pytest = "*" } }
        "#;

        let toplevel = TopLevel::from_toml_str(input).unwrap();
        assert_matches!(toplevel.env, TomlEnvironmentList::Map(env) => {
            assert!(env.has_inline_config());
            assert!(env.features.is_some());
            assert!(env.dependencies.is_some());
        });
    }

    #[test]
    pub fn test_parse_inline_tasks() {
        let input = r#"
            [env]
            dependencies = { pytest = "*" }

            [env.tasks]
            test = "pytest"
        "#;

        let toplevel = TopLevel::from_toml_str(input).unwrap();
        assert_matches!(toplevel.env, TomlEnvironmentList::Map(env) => {
            assert!(env.has_inline_config());
            assert!(!env.tasks.is_empty());
        });
    }

    #[test]
    pub fn test_inline_only_no_features_required() {
        // When inline config is present, features field is not required
        let input = r#"
            env = { dependencies = { git = "*" } }
        "#;

        let toplevel = TopLevel::from_toml_str(input).unwrap();
        assert_matches!(toplevel.env, TomlEnvironmentList::Map(env) => {
            assert!(env.features.is_none());
            assert!(env.has_inline_config());
        });
    }

    #[test]
    pub fn test_into_toml_feature_with_inline_config() {
        let input = r#"
            env = { dependencies = { pytest = "*" }, channels = ["conda-forge"] }
        "#;

        let toplevel = TopLevel::from_toml_str(input).unwrap();
        assert_matches!(toplevel.env, TomlEnvironmentList::Map(env) => {
            assert!(env.has_inline_config());
            let toml_feature = env.into_toml_feature();
            assert!(toml_feature.is_some());
            let feature = toml_feature.unwrap();
            assert!(feature.dependencies.is_some());
            assert!(feature.channels.is_some());
        });
    }

    #[test]
    pub fn test_into_toml_feature_without_inline_config() {
        let input = r#"
            env = { features = ["foo"], solve-group = "group" }
        "#;

        let toplevel = TopLevel::from_toml_str(input).unwrap();
        assert_matches!(toplevel.env, TomlEnvironmentList::Map(env) => {
            assert!(!env.has_inline_config());
            let toml_feature = env.into_toml_feature();
            assert!(toml_feature.is_none());
        });
    }
}
