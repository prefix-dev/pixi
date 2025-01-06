use std::collections::HashMap;

use indexmap::IndexMap;
use pixi_spec::PixiSpec;
use pixi_toml::{TomlHashMap, TomlIndexMap};
use toml_span::{de_helpers::TableHelper, DeserError, Value};

use crate::{
    error::FeatureNotEnabled,
    pypi::PyPiPackageName,
    target::PackageTarget,
    utils::{package_map::UniquePackageMap, PixiSpanned},
    Activation, KnownPreviewFeature, Preview, PyPiRequirement, SpecType, Task, TaskName, TomlError,
    WorkspaceTarget,
};

#[derive(Debug, Default)]
pub struct TomlTarget {
    pub dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub host_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub run_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub pypi_dependencies: Option<IndexMap<PyPiPackageName, PyPiRequirement>>,

    /// Additional information to activate an environment.
    pub activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    pub tasks: HashMap<TaskName, Task>,
}

impl TomlTarget {
    /// Called to convert this instance into a workspace and optional package
    /// target. Based on whether `pixi-build` is enabled a different path is
    /// used.
    pub fn into_top_level_targets(
        self,
        preview: &Preview,
    ) -> Result<(WorkspaceTarget, Option<PackageTarget>), TomlError> {
        let pixi_build_enabled = preview.is_enabled(KnownPreviewFeature::PixiBuild);

        if pixi_build_enabled {
            self.into_workspace_and_package_targets()
        } else {
            Ok((self.into_workspace_target()?, None))
        }
    }

    /// Called to convert this instance into a workspace target of a feature.
    pub fn into_feature_target(self, preview: &Preview) -> Result<WorkspaceTarget, TomlError> {
        let pixi_build_enabled = preview.is_enabled(KnownPreviewFeature::PixiBuild);

        if pixi_build_enabled {
            if let Some(run_dependencies) = self.run_dependencies {
                return Err(TomlError::Generic(
                    "[run-dependencies] in features are not supported.".into(),
                    run_dependencies.span,
                ));
            }

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

    /// Called to convert this instance into a workspace and optional package
    /// target.
    fn into_workspace_and_package_targets(
        self,
    ) -> Result<(WorkspaceTarget, Option<PackageTarget>), TomlError> {
        let workspace_target = WorkspaceTarget {
            dependencies: combine_target_dependencies([(SpecType::Run, self.dependencies)]),
            pypi_dependencies: self.pypi_dependencies,
            activation: self.activation,
            tasks: self.tasks,
        };

        let package_dependencies = combine_target_dependencies([
            (SpecType::Run, self.run_dependencies),
            (SpecType::Host, self.host_dependencies),
            (SpecType::Build, self.build_dependencies),
        ]);

        let package_target = if package_dependencies.is_empty() {
            None
        } else {
            Some(PackageTarget {
                dependencies: package_dependencies,
            })
        };

        Ok((workspace_target, package_target))
    }

    /// Called when parsing the manifest as a pre-pixi-build manifest.
    fn into_workspace_target(self) -> Result<WorkspaceTarget, TomlError> {
        if let Some(run_dependencies) = self.run_dependencies {
            return Err(TomlError::FeatureNotEnabled(
                FeatureNotEnabled::new(
                    "[run-dependencies] are only available when using the `pixi-build` feature.",
                    KnownPreviewFeature::PixiBuild,
                )
                .with_opt_span(run_dependencies.span),
            ));
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
        let run_dependencies = th.optional("run-dependencies");
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
            run_dependencies,
            pypi_dependencies,
            activation,
            tasks,
        })
    }
}
