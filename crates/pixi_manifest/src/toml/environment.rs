use std::{collections::HashMap, path::Path};

use indexmap::{IndexMap, IndexSet};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_toml::{Same, TomlHashMap, TomlIndexMap, TomlIndexSet, TomlWith};
use toml_span::{DeserError, Spanned, Value, de_helpers::expected};

use crate::{
    Activation, Feature, FeatureName, SystemRequirements, TargetSelector, Task, TaskName,
    TomlError, Warning, WithWarnings,
    pypi::pypi_options::PypiOptions,
    toml::{
        TomlFeature, TomlPrioritizedChannel, TomlTarget, TomlWorkspace, WorkspacePackageProperties,
        preview::TomlPreview, task::TomlTask,
    },
    utils::{
        PixiSpanned,
        package_map::{DependencyTable, UniquePackageMap},
    },
    workspace::{ChannelPriority, SolveStrategy},
};

/// Helper struct to deserialize the environment from TOML.
/// The environment description can only hold these values.
#[derive(Debug)]
pub struct TomlEnvironment {
    pub features: Option<Spanned<Vec<Spanned<String>>>>,
    pub solve_group: Option<String>,
    pub no_default_feature: bool,
    /// Feature content defined directly on the environment. This is turned into
    /// an implicit feature that is prepended to the environment's features.
    pub inline: TomlEnvironmentInline,
}

/// The feature content that can be defined directly on an environment. This is
/// the same content as a regular feature, minus the fields that only make sense
/// on a feature (`host-dependencies`, `build-dependencies` and
/// `system-requirements`).
#[derive(Debug, Default)]
pub struct TomlEnvironmentInline {
    pub platforms: Option<Spanned<IndexSet<String>>>,
    pub channels: Option<Vec<TomlPrioritizedChannel>>,
    pub channel_priority: Option<ChannelPriority>,
    pub solve_strategy: Option<SolveStrategy>,
    pub target: IndexMap<PixiSpanned<TargetSelector>, TomlTarget>,
    pub dependencies: Option<PixiSpanned<DependencyTable>>,
    pub pypi_dependencies: Option<IndexMap<PypiPackageName, PixiPypiSpec>>,
    pub dev_dependencies:
        Option<IndexMap<rattler_conda_types::PackageName, pixi_spec::TomlLocationSpec>>,
    pub constraints: Option<PixiSpanned<UniquePackageMap>>,
    pub activation: Option<Activation>,
    pub tasks: HashMap<TaskName, Task>,
    pub pypi_options: Option<PypiOptions>,
    pub warnings: Vec<Warning>,
}

impl TomlEnvironmentInline {
    /// Returns true if the environment does not define any inline feature
    /// content, in which case no implicit feature needs to be synthesized.
    pub fn is_empty(&self) -> bool {
        let Self {
            platforms,
            channels,
            channel_priority,
            solve_strategy,
            target,
            dependencies,
            pypi_dependencies,
            dev_dependencies,
            constraints,
            activation,
            tasks,
            pypi_options,
            warnings: _,
        } = self;
        platforms.is_none()
            && channels.is_none()
            && channel_priority.is_none()
            && solve_strategy.is_none()
            && target.is_empty()
            && dependencies.is_none()
            && pypi_dependencies.is_none()
            && dev_dependencies.is_none()
            && constraints.is_none()
            && activation.is_none()
            && tasks.is_empty()
            && pypi_options.is_none()
    }

    /// Builds the implicit feature that carries the environment's inline
    /// content.
    pub fn into_feature(
        self,
        name: FeatureName,
        preview: &TomlPreview,
        workspace: &TomlWorkspace,
        workspace_package_properties: &WorkspacePackageProperties,
        root_directory: &Path,
    ) -> Result<WithWarnings<Feature>, TomlError> {
        let Self {
            platforms,
            channels,
            channel_priority,
            solve_strategy,
            target,
            dependencies,
            pypi_dependencies,
            dev_dependencies,
            constraints,
            activation,
            tasks,
            pypi_options,
            warnings,
        } = self;
        let WithWarnings {
            value: (feature, _system_requirements),
            warnings,
        } = TomlFeature {
            platforms,
            channels,
            channel_priority,
            solve_strategy,
            target,
            dependencies,
            pypi_dependencies,
            dev: dev_dependencies,
            constraints,
            activation,
            tasks,
            pypi_options,
            host_dependencies: None,
            build_dependencies: None,
            system_requirements: SystemRequirements::default(),
            warnings,
        }
        .into_feature(
            name,
            preview,
            workspace,
            workspace_package_properties,
            root_directory,
        )?;
        Ok(WithWarnings::from(feature).with_warnings(warnings))
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

        let features = th.optional_s("features");
        let solve_group = th.optional("solve-group");
        let no_default_feature = th.optional("no-default-feature");

        // Inline feature content. `host-dependencies`, `build-dependencies` and
        // `system-requirements` are intentionally not accepted here and are
        // rejected by `finalize` below.
        let platforms = th
            .optional::<TomlWith<_, Spanned<TomlIndexSet<Same>>>>("platforms")
            .map(TomlWith::into_inner);
        let channels = th.optional("channels");
        let channel_priority = th.optional("channel-priority");
        let solve_strategy = th.optional("solve-strategy");
        let target = th
            .optional::<TomlIndexMap<_, _>>("target")
            .map(TomlIndexMap::into_inner)
            .unwrap_or_default();
        let dependencies = th.optional("dependencies");
        let pypi_dependencies = th
            .optional::<TomlIndexMap<_, _>>("pypi-dependencies")
            .map(TomlIndexMap::into_inner);
        let dev_dependencies = th
            .optional::<TomlIndexMap<_, _>>("dev")
            .map(TomlIndexMap::into_inner);
        let constraints = th.optional("constraints");
        let activation = th.optional("activation");
        let tasks = th
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
        let pypi_options = th.optional("pypi-options");

        th.finalize(None)?;

        let inline = TomlEnvironmentInline {
            platforms,
            channels,
            channel_priority,
            solve_strategy,
            target,
            dependencies,
            pypi_dependencies,
            dev_dependencies,
            constraints,
            activation,
            tasks,
            pypi_options,
            warnings,
        };

        if features.is_none() && solve_group.is_none() && inline.is_empty() {
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
            inline,
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
}
