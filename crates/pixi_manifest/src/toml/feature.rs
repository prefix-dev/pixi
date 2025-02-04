use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use pixi_toml::{TomlHashMap, TomlIndexMap, TomlIndexSet, TomlWith};
use rattler_conda_types::Platform;
use toml_span::{de_helpers::TableHelper, DeserError, Spanned, Value};

use crate::{
    pypi::{pypi_options::PypiOptions, PyPiPackageName},
    toml::{
        create_unsupported_selector_error, platform::TomlPlatform, preview::TomlPreview,
        task::TomlTask, PlatformSpan, TomlPrioritizedChannel, TomlTarget, TomlWorkspace,
    },
    utils::{package_map::UniquePackageMap, PixiSpanned},
    workspace::ChannelPriority,
    Activation, Feature, FeatureName, PyPiRequirement, SystemRequirements, TargetSelector, Targets,
    Task, TaskName, TomlError, Warning, WithWarnings,
};

#[derive(Debug)]
pub struct TomlFeature {
    pub platforms: Option<Spanned<IndexSet<Platform>>>,
    pub channels: Option<Vec<TomlPrioritizedChannel>>,
    pub channel_priority: Option<ChannelPriority>,
    pub system_requirements: SystemRequirements,
    pub target: IndexMap<PixiSpanned<TargetSelector>, TomlTarget>,
    pub dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub host_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub pypi_dependencies: Option<IndexMap<PyPiPackageName, PyPiRequirement>>,

    /// Additional information to activate an environment.
    pub activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    pub tasks: HashMap<TaskName, Task>,

    /// Additional options for PyPi dependencies.
    pub pypi_options: Option<PypiOptions>,

    /// Any warnings we encountered while parsing the feature
    pub warnings: Vec<Warning>,
}

impl TomlFeature {
    pub fn into_feature(
        self,
        name: FeatureName,
        preview: &TomlPreview,
        workspace: &TomlWorkspace,
    ) -> Result<WithWarnings<Feature>, TomlError> {
        let WithWarnings {
            value: default_target,
            mut warnings,
        } = TomlTarget {
            dependencies: self.dependencies,
            host_dependencies: self.host_dependencies,
            build_dependencies: self.build_dependencies,
            pypi_dependencies: self.pypi_dependencies,
            activation: self.activation,
            tasks: self.tasks,
            warnings: self.warnings,
        }
        .into_workspace_target(None, preview)?;

        let mut targets = IndexMap::new();
        for (selector, target) in self.target {
            // Verify that the target selector matches at least one of the platforms of the
            // feature and/or workspace.
            let matching_platforms = Platform::all()
                .filter(|p| selector.value.matches(*p))
                .collect::<Vec<_>>();

            if let Some(feature_platforms) = self.platforms.as_ref() {
                if !matching_platforms
                    .iter()
                    .any(|p| feature_platforms.value.contains(p))
                {
                    return Err(create_unsupported_selector_error(
                        PlatformSpan::Feature(name.to_string(), feature_platforms.span),
                        &selector,
                        &matching_platforms,
                    )
                    .into());
                }
            } else if !matching_platforms
                .iter()
                .any(|p| workspace.platforms.value.contains(p))
            {
                return Err(create_unsupported_selector_error(
                    PlatformSpan::Workspace(workspace.platforms.span),
                    &selector,
                    &matching_platforms,
                )
                .into());
            }

            let WithWarnings {
                value: target,
                warnings: mut target_warnings,
            } = target.into_workspace_target(Some(selector.value.clone()), preview)?;
            targets.insert(selector, target);
            warnings.append(&mut target_warnings);
        }

        Ok(WithWarnings::from(Feature {
            name,
            platforms: self.platforms.map(|platforms| platforms.value),
            channels: self
                .channels
                .map(|channels| channels.into_iter().map(|channel| channel.into()).collect()),
            channel_priority: self.channel_priority,
            system_requirements: self.system_requirements,
            pypi_options: self.pypi_options,
            targets: Targets::from_default_and_user_defined(default_target, targets),
        })
        .with_warnings(warnings))
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlFeature {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let mut warnings = Vec::new();

        let platforms = th
            .optional::<TomlWith<_, Spanned<TomlIndexSet<TomlPlatform>>>>("platforms")
            .map(TomlWith::into_inner);
        let channels = th.optional("channels");
        let channel_priority = th.optional("channel-priority");
        let target = th
            .optional::<TomlIndexMap<_, _>>("target")
            .map(TomlIndexMap::into_inner)
            .unwrap_or_default();
        let dependencies = th.optional("dependencies");
        let host_dependencies = th.optional("host-dependencies");
        let build_dependencies = th.optional("build-dependencies");
        let pypi_dependencies = th
            .optional::<TomlIndexMap<_, _>>("pypi-dependencies")
            .map(TomlIndexMap::into_inner);
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
        let system_requirements = th.optional("system-requirements").unwrap_or_default();

        th.finalize(None)?;

        Ok(TomlFeature {
            platforms,
            channels,
            channel_priority,
            system_requirements,
            target,
            dependencies,
            host_dependencies,
            build_dependencies,
            pypi_dependencies,
            activation,
            tasks,
            pypi_options,
            warnings,
        })
    }
}

#[cfg(test)]
mod test {
    use insta::assert_snapshot;

    use crate::utils::test_utils::expect_parse_failure;

    #[test]
    fn test_mismatching_target_selector() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = ['win-64']

        [feature.foo.target.osx-64.dependencies]
        "#,
        ));
    }

    #[test]
    fn test_mismatching_excluded_target_selector() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = ['win-64', 'osx-arm64']

        [feature.foo]
        platforms = ['win-64']

        [feature.foo.target.osx.dependencies]
        "#,
        ));
    }
}
