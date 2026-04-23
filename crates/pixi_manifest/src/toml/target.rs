use std::collections::HashMap;
use std::path::Path;

use indexmap::IndexMap;
use pixi_spec::{PixiSpec, SourceSpec, TomlLocationSpec};
use pixi_spec_containers::DependencyMap;
use pixi_toml::{TomlHashMap, TomlIndexMap};
use toml_span::{DeserError, Value, de_helpers::TableHelper};

use crate::{
    Activation, InternalDependencyBehavior, KnownPreviewFeature, SpecType, TargetSelector, Task,
    TaskName, TomlError, Warning, WithWarnings, WorkspaceTarget,
    error::GenericError,
    pypi_txt_expand::expand_pypi_txt_paths_blocking,
    toml::{
        conda_dependency_table::CondaDependencyTable, preview::TomlPreview, task::TomlTask,
    },
    utils::{PixiSpanned, package_map::UniquePackageMap},
};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use rattler_conda_types::PackageName;

#[derive(Debug, Default)]
pub struct TomlTarget {
    pub dependencies: Option<PixiSpanned<CondaDependencyTable>>,
    pub host_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub pypi_dependencies: Option<IndexMap<PypiPackageName, PixiPypiSpec>>,
    pub dev_dependencies: Option<IndexMap<PackageName, TomlLocationSpec>>,

    /// Version constraints - limit versions of packages that can be installed
    /// without explicitly requiring them.
    pub constraints: Option<PixiSpanned<UniquePackageMap>>,

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
        manifest_dir: Option<&Path>,
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
                            Some(selector) => format!("Did you mean [package.target.{selector}.host-dependencies]?"),
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
                            Some(selector) => format!("Did you mean [package.target.{selector}.build-dependencies]?"),
                        })
                        .with_opt_label("pixi-build is enabled here", preview.get_span(KnownPreviewFeature::PixiBuild))
                ));
            }
        }

        // Convert dev dependencies from TomlLocationSpec to SourceSpec
        let dev_dependencies = self
            .dev_dependencies
            .map(|dev_map| {
                dev_map
                    .into_iter()
                    .map(|(name, toml_loc)| {
                        toml_loc
                            .into_source_location_spec()
                            .map(|location| (name, SourceSpec::from(location)))
                    })
                    .collect::<Result<IndexMap<_, _>, _>>()
            })
            .transpose()
            .map_err(|e| {
                TomlError::Generic(GenericError::new(format!(
                    "failed to parse dev dependency: {e}",
                )))
            })?;

        // Convert constraints from UniquePackageMap to DependencyMap.
        // Source specs are never valid in [constraints], regardless of pixi-build mode.
        let constraints = self
            .constraints
            .map(|c| {
                if let Some((name, _)) = c.value.specs.iter().find(|(_, spec)| spec.is_source()) {
                    return Err(TomlError::Generic(
                        GenericError::new(format!(
                            "source specifications are not supported in `[constraints]`, but '{}' is a source specification",
                            name.as_source()
                        ))
                        .with_opt_span(c.value.value_spans.get(name).cloned())
                        .with_span_label("source specification specified here")
                        .with_help(
                            "constraints only apply to packages resolved from channels, not source packages",
                        ),
                    ));
                }
                c.value
                    .into_inner(pixi_build_enabled)
                    .map(|index_map| index_map.into_iter().collect())
            })
            .transpose()?;

        let (run_dependencies, pypi_txt_paths) = match self.dependencies {
            Some(spanned) => {
                let span = spanned.span;
                let (conda, paths) = spanned.value.into_spanned_unique_map(span);
                (conda, paths)
            }
            None => (None, Vec::new()),
        };

        let mut workspace_target = WorkspaceTarget {
            dependencies: combine_target_dependencies(
                [
                    (SpecType::Run, run_dependencies),
                    (SpecType::Host, self.host_dependencies),
                    (SpecType::Build, self.build_dependencies),
                ],
                pixi_build_enabled,
            )?,
            pypi_dependencies: self.pypi_dependencies.map(|index_map| {
                // Convert IndexMap to DependencyMap
                index_map.into_iter().collect()
            }),
            dev_dependencies: dev_dependencies.map(|index_map| {
                // Convert IndexMap to DependencyMap
                index_map.into_iter().collect()
            }),
            constraints,
            activation: self.activation,
            tasks: self.tasks,
        };

        if !pypi_txt_paths.is_empty() {
            let Some(manifest_dir) = manifest_dir else {
                return Err(TomlError::Generic(
                    GenericError::new(
                        "the `pypi-txt` key in `[dependencies]` requires a manifest directory",
                    )
                    .with_help(
                        "This usually means the manifest was parsed without a path on disk; use a file-backed manifest.",
                    ),
                ));
            };
            let expanded = expand_pypi_txt_paths_blocking(&pypi_txt_paths, manifest_dir)?;
            // Inline `[pypi-dependencies]` were converted above; append `pypi-txt` specs so both
            // sources coexist (alternate specs for the same package name are merged by DependencyMap).
            for req in expanded {
                let name = PypiPackageName::from_normalized(req.name.clone());
                let spec = PixiPypiSpec::try_from(req).map_err(|e| {
                    TomlError::Generic(GenericError::new(format!(
                        "failed to convert dependency from `pypi-txt`: {e}"
                    )))
                })?;
                workspace_target.add_pypi_dependency(name, spec, InternalDependencyBehavior::Append);
            }
        }

        Ok(WithWarnings {
            value: workspace_target,
            warnings: self.warnings,
        })
    }
}

/// Combines different target dependencies into a single map.
pub(super) fn combine_target_dependencies(
    iter: impl IntoIterator<Item = (SpecType, Option<PixiSpanned<UniquePackageMap>>)>,
    is_pixi_build_enabled: bool,
) -> Result<HashMap<SpecType, DependencyMap<PackageName, PixiSpec>>, TomlError> {
    iter.into_iter()
        .filter_map(|(ty, deps)| {
            deps.map(|deps| {
                deps.value
                    .into_inner(is_pixi_build_enabled)
                    .map(|index_map| {
                        // Convert IndexMap to DependencyMap
                        let dep_map: DependencyMap<PackageName, PixiSpec> =
                            index_map.into_iter().collect();
                        (ty, dep_map)
                    })
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
            constraints,
            pypi_dependencies,
            dev_dependencies: dev,
            activation,
            tasks,
            warnings,
        })
    }
}
