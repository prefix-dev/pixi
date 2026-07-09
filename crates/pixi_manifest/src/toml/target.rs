use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    path::Path,
};

use indexmap::IndexMap;
use itertools::Either;
use pixi_spec::{PixiSpec, TomlSpec};
use pixi_spec_containers::DependencyMap;
use pixi_toml::{TomlHashMap, TomlIndexMap};
use toml_span::{DeserError, Value, de_helpers::TableHelper};
use xxhash_rust::xxh3::Xxh3;

use crate::{
    Activation, InlineContentHash, InlinePackageManifest, KnownPreviewFeature, SpecType,
    TargetSelector, Task, TaskName, TomlError, Warning, WithWarnings, WorkspaceTarget,
    error::GenericError,
    toml::{
        PackageDefaults, TomlPackage, WorkspacePackageProperties, preview::TomlPreview,
        task::TomlTask,
    },
    utils::{
        PixiSpanned,
        package_map::{DependencyTable, UniquePackageMap},
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
    pub dev_dependencies: Option<IndexMap<PackageName, TomlSpec>>,

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
    ///
    /// `root_directory` is the directory of the manifest that defines this
    /// target; it is used to resolve and convert any inline package
    /// definitions. `workspace_package_properties` are the consuming workspace's
    /// values that inline package definitions inherit from when they use
    /// `{ workspace = true }`.
    pub fn into_workspace_target(
        self,
        target: Option<TargetSelector>,
        preview: &TomlPreview,
        workspace_package_properties: &WorkspacePackageProperties,
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

        // Peel inline package definitions off each consumer dependency table,
        // leaving the source specs to flow into the regular dependency map.
        let mut inline_toml: IndexMap<PackageName, PixiSpanned<TomlPackage>> = IndexMap::new();
        let dependencies = split_inline_packages(dependencies, &mut inline_toml)?;
        let host_dependencies = split_inline_packages(host_dependencies, &mut inline_toml)?;
        let build_dependencies = split_inline_packages(build_dependencies, &mut inline_toml)?;

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
                value: manifest,
                warnings: mut package_warnings,
            } = package.value.into_manifest(
                workspace_package_properties.clone(),
                PackageDefaults::default(),
                &full_preview,
                root_directory,
            )?;
            warnings.append(&mut package_warnings);

            // Fingerprint the assembled manifest so editing the inline
            // definition invalidates the content-addressed build caches it
            // feeds. The dependency name is folded in so two identical inline
            // tables declared under different names stay distinct.
            let content_hash = {
                let mut hasher = Xxh3::new();
                name.as_normalized().hash(&mut hasher);
                manifest.hash(&mut hasher);
                InlineContentHash(hasher.finish())
            };

            inline_packages.insert(
                name,
                InlinePackageManifest {
                    manifest,
                    content_hash,
                },
            );
        }

        // Convert dev dependencies. A dev dependency is a source (path/git/url)
        // spec that may additionally carry an `extras` selector to pull in the
        // package's extra-dependency groups. Other matchspec selectors are not
        // (yet) meaningful for a dev dependency and are rejected with a clear
        // message rather than being silently ignored.
        let dev_dependencies = dev_dependencies
            .map(|dev_map| {
                dev_map
                    .into_iter()
                    .map(|(name, toml_spec)| {
                        let spec = toml_spec.into_spec().map_err(|e| {
                            TomlError::Generic(GenericError::new(format!(
                                "failed to parse dev dependency '{}': {e}",
                                name.as_source()
                            )))
                        })?;
                        let source_spec = match spec.into_source_or_binary() {
                            Either::Left(source_spec) => source_spec,
                            Either::Right(_) => {
                                return Err(TomlError::Generic(
                                    GenericError::new(format!(
                                        "the dev dependency '{}' is not a source dependency",
                                        name.as_source()
                                    ))
                                    .with_help(
                                        "dev dependencies must refer to a source package using `path`, `git` or `url`",
                                    ),
                                ));
                            }
                        };
                        validate_dev_dependency_matchspec(&name, &source_spec.matchspec)?;
                        Ok((name, source_spec))
                    })
                    .collect::<Result<IndexMap<_, _>, TomlError>>()
            })
            .transpose()?;

        // Convert constraints from UniquePackageMap to DependencyMap.
        // Source specs are never valid in [constraints], regardless of pixi-build mode.
        let constraints = constraints
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

/// Splits a consumer dependency table into its source specs and inline package
/// definitions, draining the latter into `inline`. Errors if a package name
/// already has an inline definition in another table.
fn split_inline_packages(
    table: Option<PixiSpanned<DependencyTable>>,
    inline: &mut IndexMap<PackageName, PixiSpanned<TomlPackage>>,
) -> Result<Option<PixiSpanned<UniquePackageMap>>, TomlError> {
    let Some(PixiSpanned { span, value }) = table else {
        return Ok(None);
    };
    let DependencyTable {
        specs,
        inline_packages,
    } = value;
    for (name, package) in inline_packages {
        if inline.insert(name.clone(), package).is_some() {
            return Err(TomlError::Generic(GenericError::new(format!(
                "the package '{}' has more than one inline definition",
                name.as_source()
            ))));
        }
    }
    Ok(Some(PixiSpanned { span, value: specs }))
}

/// Validate that a dev dependency only carries selectors that are meaningful
/// for a source package whose dependencies (not the package itself) are
/// installed. Currently only `extras` is supported; any other matchspec
/// selector is rejected with an actionable error instead of being silently
/// dropped.
fn validate_dev_dependency_matchspec(
    name: &PackageName,
    matchspec: &pixi_spec::MatchspecFields,
) -> Result<(), TomlError> {
    let unsupported = [
        ("version", matchspec.version.is_some()),
        ("build", matchspec.build.is_some()),
        ("build-number", matchspec.build_number.is_some()),
        ("flags", matchspec.flags.is_some()),
        ("subdir", matchspec.subdir.is_some()),
        ("license", matchspec.license.is_some()),
        ("when", matchspec.condition.is_some()),
        ("track-features", matchspec.track_features.is_some()),
    ]
    .into_iter()
    .find_map(|(field, is_set)| is_set.then_some(field));

    if let Some(field) = unsupported {
        return Err(TomlError::Generic(
            GenericError::new(format!(
                "the `{field}` field is not supported for the dev dependency '{}'",
                name.as_source()
            ))
            .with_help(
                "only `extras` is supported alongside the source location in the `dev` table",
            ),
        ));
    }

    Ok(())
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
