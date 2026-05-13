use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use pixi_toml::{Same, TomlHashMap, TomlIndexMap, TomlIndexSet, TomlWith};
use toml_span::{DeserError, Spanned, Value, de_helpers::TableHelper};

use crate::{
    Activation, Feature, FeatureName, PixiPlatformName, SystemRequirements, TargetSelector,
    Targets, Task, TaskName, TomlError, Warning, WithWarnings,
    pypi::pypi_options::PypiOptions,
    toml::{
        PlatformSpan, TomlPrioritizedChannel, TomlTarget, TomlWorkspace,
        create_unsupported_selector_warning, preview::TomlPreview, task::TomlTask,
    },
    utils::{PixiSpanned, package_map::UniquePackageMap},
    warning::Deprecation,
    workspace::{ChannelPriority, SolveStrategy},
};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};

#[derive(Debug)]
pub struct TomlFeature {
    pub platforms: Option<Spanned<IndexSet<String>>>,
    pub channels: Option<Vec<TomlPrioritizedChannel>>,
    pub channel_priority: Option<ChannelPriority>,
    pub system_requirements: SystemRequirements,
    pub target: IndexMap<PixiSpanned<TargetSelector>, TomlTarget>,
    pub solve_strategy: Option<SolveStrategy>,
    pub dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub host_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub pypi_dependencies: Option<IndexMap<PypiPackageName, PixiPypiSpec>>,
    pub dev: Option<IndexMap<rattler_conda_types::PackageName, pixi_spec::TomlLocationSpec>>,

    /// Version constraints - limit versions of packages that can be installed
    /// without explicitly requiring them.
    pub constraints: Option<PixiSpanned<UniquePackageMap>>,

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
            constraints: self.constraints,
            pypi_dependencies: self.pypi_dependencies,
            dev_dependencies: self.dev,
            activation: self.activation,
            tasks: self.tasks,
            warnings: self.warnings,
        }
        .into_workspace_target(None, preview)?;

        let feature_platform_names = self
            .platforms
            .map(|p| {
                match p
                    .value
                    .iter()
                    .map(|name| {
                        PixiPlatformName::try_from(name.as_str())
                            .map_err(|_| TomlError::InvalidPlatform(name.clone()))
                    })
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(value) => Ok(Spanned::with_span(value, p.span)),
                    Err(e) => Err(e),
                }
            })
            .transpose()?;

        let known_workspace_platforms = &workspace.platforms.value;

        let mut targets = IndexMap::new();
        for (selector, target) in self.target {
            // Verify that the target selector matches at least one of the platforms of the
            // feature and/or workspace.
            let matching_platforms = known_workspace_platforms
                .iter()
                .filter(|p| selector.value.matches(p))
                .collect::<Vec<_>>();

            if matching_platforms.is_empty() {
                // The *selector* did not match any of the platforms defined in the Workspace
                let warning = create_unsupported_selector_warning(
                    PlatformSpan::Workspace(workspace.platforms.span),
                    &selector,
                    &matching_platforms,
                );
                warnings.push(warning.into());
            } else if let Some(feature_platforms) = feature_platform_names.as_ref()
                && !matching_platforms
                    .iter()
                    .any(|p| feature_platforms.value.iter().any(|fp| fp == p.name()))
            {
                let warning = create_unsupported_selector_warning(
                    PlatformSpan::Feature(name.to_string(), feature_platforms.span),
                    &selector,
                    &matching_platforms,
                );
                warnings.push(warning.into());
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
            platforms: feature_platform_names
                .map(|spnv| spnv.value)
                .map(|pnv| pnv.into_iter().collect()),
            channels: self
                .channels
                .map(|channels| channels.into_iter().map(|channel| channel.into()).collect()),
            channel_priority: self.channel_priority,
            solve_strategy: self.solve_strategy,
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

        let constraints = th.optional("constraints");
        let pypi_dependencies = th
            .optional::<TomlIndexMap<_, _>>("pypi-dependencies")
            .map(TomlIndexMap::into_inner);
        let dev = th
            .optional::<TomlIndexMap<_, _>>("dev")
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
            solve_strategy,
            system_requirements,
            target,
            dependencies,
            host_dependencies,
            build_dependencies,
            constraints,
            pypi_dependencies,
            dev,
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

    use crate::utils::test_utils::expect_parse_warnings;

    #[test]
    fn test_mismatching_target_selector() {
        assert_snapshot!(expect_parse_warnings(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ['win-64']

        [feature.foo.target.osx-64.dependencies]
        "#,
        ));
    }

    #[test]
    fn test_mismatching_excluded_target_selector() {
        assert_snapshot!(expect_parse_warnings(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ['win-64', 'osx-arm64']

        [feature.foo]
        platforms = ['win-64']

        [feature.foo.target.osx.dependencies]
        "#,
        ));
    }

    #[test]
    fn test_host_dependencies_deprecation_warning() {
        assert_snapshot!(
            expect_parse_warnings(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ['linux-64']

        [feature.foo.host-dependencies]
        foo = "*"

        [environments]
        dev = ["foo"]
        "#,
            ),
            @r#"
         ⚠ The `host-dependencies` field is deprecated. Use `dependencies` instead.
          ╭─[pixi.toml:7:9]
        6 │
        7 │ ╭─▶         [feature.foo.host-dependencies]
        8 │ ├─▶         foo = "*"
          · ╰──── replace this with 'dependencies'
        9 │
          ╰────
        "#
        );
    }

    #[test]
    fn test_build_dependencies_deprecation_warning() {
        assert_snapshot!(
            expect_parse_warnings(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ['linux-64']

        [feature.foo.build-dependencies]
        bar = "*"

        [environments]
        dev = ["foo"]
        "#,
            ),
            @r#"
         ⚠ The `build-dependencies` field is deprecated. Use `dependencies` instead.
          ╭─[pixi.toml:7:9]
        6 │
        7 │ ╭─▶         [feature.foo.build-dependencies]
        8 │ ├─▶         bar = "*"
          · ╰──── replace this with 'dependencies'
        9 │
          ╰────
        "#
        );
    }
}
