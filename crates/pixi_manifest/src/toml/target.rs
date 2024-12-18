use std::collections::HashMap;

use indexmap::IndexMap;
use pixi_spec::PixiSpec;
use pixi_toml::{TomlHashMap, TomlIndexMap};
use toml_span::{de_helpers::TableHelper, DeserError, Value};

use crate::{
    pypi::PyPiPackageName,
    utils::{package_map::UniquePackageMap, PixiSpanned},
    Activation, KnownPreviewFeature, Preview, PyPiRequirement, SpecType, Task, TaskName, TomlError,
    WorkspaceTarget,
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
}

impl TomlTarget {
    /// Called to convert this instance into a workspace target of a feature.
    pub fn into_workspace_target(self, preview: &Preview) -> Result<WorkspaceTarget, TomlError> {
        let pixi_build_enabled = preview.is_enabled(KnownPreviewFeature::PixiBuild);

        if pixi_build_enabled {
            if let Some(host_dependencies) = self.host_dependencies {
                return Err(TomlError::Generic(
                    "[host-dependencies] in features are not supported when `pixi-build` is enabled."
                        .into(),
                    host_dependencies.span,
                ));
            }

            if let Some(build_dependencies) = self.build_dependencies {
                return Err(TomlError::Generic(
                    "[build-dependencies] in features are not supported when `pixi-build` is enabled."
                        .into(),
                    build_dependencies.span,
                ));
            }
        }

        Ok(WorkspaceTarget {
            dependencies: combine_target_dependencies([
                (SpecType::Run, self.dependencies),
                (SpecType::Host, self.host_dependencies),
                (SpecType::Build, self.build_dependencies),
            ]),
            pypi_dependencies: self.pypi_dependencies,
            activation: self.activation,
            tasks: self.tasks,
        })
    }
}

/// Combines different target dependencies into a single map.
pub(super) fn combine_target_dependencies(
    iter: impl IntoIterator<Item = (SpecType, Option<PixiSpanned<UniquePackageMap>>)>,
) -> HashMap<SpecType, IndexMap<rattler_conda_types::PackageName, PixiSpec>> {
    iter.into_iter()
        .filter_map(|(ty, deps)| deps.map(|deps| (ty, deps.value.into())))
        .collect()
}

impl<'de> toml_span::Deserialize<'de> for TomlTarget {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let dependencies = th.optional("dependencies");
        let host_dependencies = th.optional("host-dependencies");
        let build_dependencies = th.optional("build-dependencies");
        let pypi_dependencies = th
            .optional::<TomlIndexMap<_, _>>("pypi-dependencies")
            .map(TomlIndexMap::into_inner);
        let activation = th.optional("activation");
        let tasks = th
            .optional::<TomlHashMap<_, _>>("tasks")
            .map(TomlHashMap::into_inner)
            .unwrap_or_default();

        th.finalize(None)?;

        Ok(TomlTarget {
            dependencies,
            host_dependencies,
            build_dependencies,
            pypi_dependencies,
            activation,
            tasks,
        })
    }
}
