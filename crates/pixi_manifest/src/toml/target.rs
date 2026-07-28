use std::{collections::HashMap, path::Path};

use indexmap::IndexMap;
use pixi_spec::{PixiSpec, TomlLocationSpec, TomlSpec};
use pixi_spec_containers::DependencyMap;
use pixi_toml::{TomlHashMap, TomlIndexMap};
use toml_span::{DeserError, Value, de_helpers::TableHelper};

use crate::{
    Activation, InlinePackageManifest, KnownPreviewFeature, SpecType, TargetSelector, Task,
    TaskName, TomlError, Warning, WithWarnings, WorkspaceTarget,
    error::GenericError,
    toml::{TomlPackage, WorkspacePackageProperties, preview::TomlPreview, task::TomlTask},
    utils::{
        PixiSpanned, inheritable_package_map::InheritablePackageMap, package_map::DependencyTable,
        package_map::UniquePackageMap,
    },
};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use rattler_conda_types::PackageName;

#[derive(Debug, Default)]
pub struct TomlTarget {
    pub dependencies: Option<PixiSpanned<DependencyTable>>,
    pub host_dependencies: Option<PixiSpanned<DependencyTable>>,
    pub build_dependencies: Option<PixiSpanned<DependencyTable>>,
    pub pypi_dependencies: Option<IndexMap<PypiPackageName, PixiPypiSpec>>,
    pub dev_dependencies: Option<IndexMap<PackageName, TomlLocationSpec>>,

    /// Version constraints - limit versions of packages that can be installed
    /// without explicitly requiring them.
    pub constraints: Option<PixiSpanned<InheritablePackageMap>>,

    /// Additional information to activate an environment.
    pub activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    pub tasks: HashMap<TaskName, Task>,

    /// Any warnings we encountered while parsing the target
    pub warnings: Vec<Warning>,
}

impl TomlTarget {
    /// Called to convert this instance into a workspace target of a feature.
    ///
    /// `root_directory` is the directory of the manifest that defines this
    /// target; it is used to resolve and convert any inline package
    /// definitions. `workspace_package_properties` are the consuming workspace's
    /// values that inline package definitions inherit from when they use
    /// `{ workspace = true }`. `workspace_dependencies` is the
    /// `[workspace.dependencies]` pool that `{ workspace = true }` dependency
    /// entries resolve against.
    pub fn into_workspace_target(
        self,
        target: Option<TargetSelector>,
        preview: &TomlPreview,
        workspace_package_properties: &WorkspacePackageProperties,
        workspace_dependencies: &IndexMap<PackageName, TomlSpec>,
        root_directory: &Path,
    ) -> Result<WithWarnings<WorkspaceTarget>, TomlError> {
        let pixi_build_enabled = preview.is_enabled(KnownPreviewFeature::PixiBuild);

        let TomlTarget {
            dependencies,
            host_dependencies,
            build_dependencies,
            pypi_dependencies,
            dev_dependencies,
            constraints,
            activation,
            tasks,
            mut warnings,
        } = self;

        if pixi_build_enabled {
            if let Some(host_dependencies) = &host_dependencies {
                return Err(TomlError::Generic(
                    GenericError::new("When `pixi-build` is enabled, host-dependencies can only be specified for a package.")
                        .with_opt_span(host_dependencies.span.clone())
                        .with_span_label("host-dependencies specified here")
                        .with_help(match target {
                            None => "Did you mean [package.host-dependencies]?".to_string(),
                            Some(selector) => format!("Did you mean [package.target.{selector}.host-dependencies]?"),
                        })
                        .with_opt_label("pixi-build is enabled here", preview.get_span(KnownPreviewFeature::PixiBuild))));
            }

            if let Some(build_dependencies) = &build_dependencies {
                return Err(TomlError::Generic(
                    GenericError::new("When `pixi-build` is enabled, build-dependencies can only be specified for a package.")
                        .with_opt_span(build_dependencies.span.clone())
                        .with_span_label("build-dependencies specified here")
                        .with_help(match target {
                            None => "Did you mean [package.build-dependencies]?".to_string(),
                            Some(selector) => format!("Did you mean [package.target.{selector}.build-dependencies]?"),
                        })
                        .with_opt_label("pixi-build is enabled here", preview.get_span(KnownPreviewFeature::PixiBuild))
                ));
            }
        }

        // Peel inline package definitions off each consumer dependency table
        // and resolve `{ workspace = true }` entries against the workspace
        // pool, leaving plain specs to flow into the regular dependency map.
        let mut inline_toml: IndexMap<PackageName, PixiSpanned<TomlPackage>> = IndexMap::new();
        let dependencies = resolve_dependency_table(
            dependencies,
            &mut inline_toml,
            workspace_dependencies,
            pixi_build_enabled,
        )?;
        let host_dependencies = resolve_dependency_table(
            host_dependencies,
            &mut inline_toml,
            workspace_dependencies,
            pixi_build_enabled,
        )?;
        let build_dependencies = resolve_dependency_table(
            build_dependencies,
            &mut inline_toml,
            workspace_dependencies,
            pixi_build_enabled,
        )?;

        // Convert the inline package definitions into full package manifests.
        // Their build source is taken from the surrounding dependency spec, so
        // the converted manifests carry no `build.source` of their own. They
        // inherit the consuming workspace's package properties, so
        // `{ workspace = true }` fields resolve as they would for an on-disk
        // `[package]`. Package defaults stay empty: an inline definition
        // describes a dependency, not the consuming project, so it must not pick
        // up the consumer's `[project]` metadata implicitly.
        let full_preview = preview.clone().into_preview().value;
        let mut inline_packages: IndexMap<PackageName, InlinePackageManifest> = IndexMap::new();
        for (name, package) in inline_toml {
            let WithWarnings {
                value: inline_manifest,
                warnings: mut package_warnings,
            } = InlinePackageManifest::from_toml_package(
                &name,
                package.value,
                workspace_package_properties.clone(),
                &full_preview,
                root_directory,
            )?;
            warnings.append(&mut package_warnings);

            inline_packages.insert(name, inline_manifest);
        }

        // Convert dev dependencies from TomlLocationSpec to SourceLocationSpec
        let dev_dependencies = dev_dependencies
            .map(|dev_map| {
                dev_map
                    .into_iter()
                    .map(|(name, toml_loc)| {
                        toml_loc
                            .into_source_location_spec()
                            .map(|location| (name, location))
                    })
                    .collect::<Result<IndexMap<_, _>, _>>()
            })
            .transpose()
            .map_err(|e| {
                TomlError::Generic(GenericError::new(format!(
                    "failed to parse dev dependency: {e}",
                )))
            })?;

        // Resolve constraints against the workspace pool and convert them to a
        // DependencyMap. Source specs are never valid in [constraints],
        // regardless of pixi-build mode, so resolution skips the preview gate
        // and the constraint-specific check below rejects them instead.
        let constraints = constraints
            .map(|c| {
                let resolved = c.value.resolve(workspace_dependencies, true)?;
                if let Some((name, _)) = resolved.specs.iter().find(|(_, spec)| spec.is_source()) {
                    return Err(TomlError::Generic(
                        GenericError::new(format!(
                            "source specifications are not supported in `[constraints]`, but '{}' is a source specification",
                            name.as_source()
                        ))
                        .with_opt_span(resolved.value_spans.get(name).cloned())
                        .with_span_label("source specification specified here")
                        .with_help(
                            "constraints only apply to packages resolved from channels, not source packages",
                        ),
                    ));
                }
                resolved
                    .into_inner(pixi_build_enabled)
                    .map(|index_map| index_map.into_iter().collect())
            })
            .transpose()?;

        Ok(WithWarnings {
            value: WorkspaceTarget {
                dependencies: combine_target_dependencies(
                    [
                        (SpecType::Run, dependencies),
                        (SpecType::Host, host_dependencies),
                        (SpecType::Build, build_dependencies),
                    ],
                    pixi_build_enabled,
                )?,
                pypi_dependencies: pypi_dependencies.map(|index_map| {
                    // Convert IndexMap to DependencyMap
                    index_map.into_iter().collect()
                }),
                dev_dependencies: dev_dependencies.map(|index_map| {
                    // Convert IndexMap to DependencyMap
                    index_map.into_iter().collect()
                }),
                inline_packages,
                constraints,
                activation,
                tasks,
            },
            warnings,
        })
    }
}

/// Resolves a consumer dependency table against the workspace pool and drains
/// its inline package definitions into `inline`. Errors if a package name
/// already has an inline definition in another table, or if an inline
/// definition is attached to an inherited entry that resolves to a binary
/// spec.
fn resolve_dependency_table(
    table: Option<PixiSpanned<DependencyTable>>,
    inline: &mut IndexMap<PackageName, PixiSpanned<TomlPackage>>,
    workspace_dependencies: &IndexMap<PackageName, TomlSpec>,
    pixi_build_enabled: bool,
) -> Result<Option<PixiSpanned<UniquePackageMap>>, TomlError> {
    let Some(PixiSpanned { span, value }) = table else {
        return Ok(None);
    };
    let DependencyTable {
        specs,
        inline_packages,
    } = value;
    let resolved = specs.resolve(workspace_dependencies, pixi_build_enabled)?;
    for (name, package) in inline_packages {
        // Direct specs were already validated at parse time; this catches
        // inherited entries whose pool spec is not a source location.
        if resolved
            .specs
            .get(&name)
            .is_some_and(|spec| !spec.is_source())
        {
            return Err(TomlError::Generic(
                GenericError::new(
                    "an inline package definition requires a `git`, `path` or `url` source location",
                )
                .with_opt_span(resolved.value_spans.get(&name).cloned()),
            ));
        }
        if inline.insert(name.clone(), package).is_some() {
            return Err(TomlError::Generic(GenericError::new(format!(
                "the package '{}' has more than one inline definition",
                name.as_source()
            ))));
        }
    }
    Ok(Some(PixiSpanned {
        span,
        value: resolved,
    }))
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
