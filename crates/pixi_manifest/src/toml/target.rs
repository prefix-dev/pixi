use std::collections::HashMap;

use indexmap::IndexMap;
use pixi_spec::PixiSpec;
use pixi_toml::{TomlHashMap, TomlIndexMap};
use toml_span::{de_helpers::TableHelper, DeserError, Value};

use crate::{
    error::GenericError,
    pypi::PyPiPackageName,
    toml::{preview::TomlPreview, task::TomlTask},
    utils::{package_map::UniquePackageMap, PixiSpanned},
    Activation, KnownPreviewFeature, PyPiRequirement, SpecType, TargetSelector, Task, TaskName,
    TomlError, Warning, WithWarnings, WorkspaceTarget,
};

#[derive(Debug, Default)]
pub struct TomlTarget {
    pub dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub host_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub pypi_dependencies: Option<IndexMap<PyPiPackageName, PyPiRequirement>>,

    /// Additional information to activate an environment.
    pub activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    pub tasks: HashMap<TaskName, Task>,

    /// Any warnings we encountered while parsing the target
    pub warnings: Vec<Warning>,
}

impl TomlTarget {
    /// Called to convert this instance into a workspace target of a feature.
    pub fn into_workspace_target(
        self,
        target: Option<TargetSelector>,
        preview: &TomlPreview,
    ) -> Result<WithWarnings<WorkspaceTarget>, TomlError> {
        let pixi_build_enabled = preview.is_enabled(KnownPreviewFeature::PixiBuild);

        if pixi_build_enabled {
            if let Some(host_dependencies) = self.host_dependencies {
                return Err(TomlError::Generic(
                    GenericError::new("When `pixi-build` is enabled, host-dependencies can only be specified for a package.")
                        .with_opt_span(host_dependencies.span)
                        .with_span_label("host-dependencies specified here")
                        .with_help(match target {
                            None => "Did you mean [package.host-dependencies]?".to_string(),
                            Some(selector) => format!("Did you mean [package.target.{}.host-dependencies]?", selector),
                        })
                        .with_opt_label("pixi-build is enabled here", preview.get_span(KnownPreviewFeature::PixiBuild))));
            }

            if let Some(build_dependencies) = self.build_dependencies {
                return Err(TomlError::Generic(
                    GenericError::new("When `pixi-build` is enabled, build-dependencies can only be specified for a package.")
                        .with_opt_span(build_dependencies.span)
                        .with_span_label("build-dependencies specified here")
                        .with_help(match target {
                            None => "Did you mean [package.build-dependencies]?".to_string(),
                            Some(selector) => format!("Did you mean [package.target.{}.build-dependencies]?", selector),
                        })
                        .with_opt_label("pixi-build is enabled here", preview.get_span(KnownPreviewFeature::PixiBuild))
                ));
            }
        }

        Ok(WithWarnings {
            value: WorkspaceTarget {
                dependencies: combine_target_dependencies(
                    [
                        (SpecType::Run, self.dependencies),
                        (SpecType::Host, self.host_dependencies),
                        (SpecType::Build, self.build_dependencies),
                    ],
                    pixi_build_enabled,
                )?,
                pypi_dependencies: self.pypi_dependencies,
                activation: self.activation,
                tasks: self.tasks,
            },
            warnings: self.warnings,
        })
    }
}

/// Combines different target dependencies into a single map.
pub(super) fn combine_target_dependencies(
    iter: impl IntoIterator<Item = (SpecType, Option<PixiSpanned<UniquePackageMap>>)>,
    is_pixi_build_enabled: bool,
) -> Result<HashMap<SpecType, IndexMap<rattler_conda_types::PackageName, PixiSpec>>, TomlError> {
    iter.into_iter()
        .filter_map(|(ty, deps)| {
            deps.map(|deps| {
                deps.value
                    .into_inner(is_pixi_build_enabled)
                    .map(|deps| (ty, deps))
            })
        })
        .collect()
}

impl<'de> toml_span::Deserialize<'de> for TomlTarget {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let mut warnings = Vec::new();

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
                let TomlTask {
                    value: task,
                    warnings: mut task_warnings,
                } = value;
                warnings.append(&mut task_warnings);
                (key, task)
            })
            .collect();

        th.finalize(None)?;

        Ok(TomlTarget {
            dependencies,
            host_dependencies,
            build_dependencies,
            pypi_dependencies,
            activation,
            tasks,
            warnings,
        })
    }
}
