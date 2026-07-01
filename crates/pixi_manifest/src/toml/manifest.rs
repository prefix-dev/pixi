use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
    str::FromStr,
};

use indexmap::{IndexMap, IndexSet};
use miette::LabeledSpan;
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::ExcludeNewer;
use pixi_toml::{Same, TomlFromStr, TomlHashMap, TomlIndexMap, TomlWith};
use rattler_conda_types::{GenericVirtualPackage, PackageName, Platform, Version};
use toml_span::{
    DeserError, Spanned, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};
use url::Url;

use crate::{
    Activation, Environment, EnvironmentName, Environments, Feature, FeatureName,
    KnownPreviewFeature, PixiPlatform, PixiPlatformName, SolveGroups, SystemRequirements,
    TargetSelector, Targets, Task, TaskName, TomlError, Warning, WithWarnings, WorkspaceManifest,
    consts,
    environment::EnvironmentIdx,
    error::{FeatureNotEnabled, GenericError},
    manifests::PackageManifest,
    pypi::pypi_options::PypiOptions,
    toml::{
        PackageDefaults, PlatformSpan, TomlFeature, TomlPackage, TomlTarget, TomlWorkspace,
        WorkspacePackageProperties, create_unsupported_selector_warning,
        environment::{TomlEnvironment, TomlEnvironmentList},
        task::TomlTask,
    },
    utils::{
        PixiSpanned,
        package_map::{DependencyTable, UniquePackageMap},
    },
    warning::Deprecation,
};

/// Raw representation of a pixi manifest. This is the deserialized form of the
/// manifest without any validation logic applied.
#[derive(Debug)]
pub struct TomlManifest {
    pub workspace: Option<PixiSpanned<TomlWorkspace>>,
    pub package: Option<PixiSpanned<TomlPackage>>,

    pub system_requirements: Option<PixiSpanned<SystemRequirements>>,
    pub target: Option<PixiSpanned<IndexMap<PixiSpanned<TargetSelector>, TomlTarget>>>,
    pub dependencies: Option<PixiSpanned<DependencyTable>>,
    pub host_dependencies: Option<PixiSpanned<DependencyTable>>,
    pub build_dependencies: Option<PixiSpanned<DependencyTable>>,

    /// Version constraints - limit versions of packages that can be installed
    /// without explicitly requiring them.
    pub constraints: Option<PixiSpanned<UniquePackageMap>>,
    pub exclude_newer: Option<PixiSpanned<IndexMap<PackageName, ExcludeNewer>>>,

    pub pypi_dependencies: Option<PixiSpanned<IndexMap<PypiPackageName, PixiPypiSpec>>>,
    pub pypi_exclude_newer: Option<PixiSpanned<IndexMap<PypiPackageName, ExcludeNewer>>>,
    pub dev_dependencies: Option<
        PixiSpanned<IndexMap<rattler_conda_types::PackageName, pixi_spec::TomlLocationSpec>>,
    >,

    /// Additional information to activate an environment.
    pub activation: Option<PixiSpanned<Activation>>,

    /// Target specific tasks to run in the environment
    pub tasks: Option<PixiSpanned<HashMap<TaskName, Task>>>,

    /// The features defined in the project.
    pub feature: Option<PixiSpanned<IndexMap<PixiSpanned<FeatureName>, TomlFeature>>>,

    /// The environments the project can create.
    pub environments: Option<PixiSpanned<IndexMap<EnvironmentName, TomlEnvironmentList>>>,

    /// pypi-options
    pub pypi_options: Option<PixiSpanned<PypiOptions>>,

    /// Any warnings we encountered while parsing the manifest
    pub warnings: Vec<Warning>,
}

impl TomlManifest {
    /// Returns true if the manifest contains a workspace.
    pub fn has_workspace(&self) -> bool {
        self.workspace.is_some()
    }

    /// Returns true if the manifest contains a package.
    pub fn has_package(&self) -> bool {
        self.package.is_some()
    }

    /// Assume that the manifest is a workspace manifest and convert it as such.
    ///
    /// If the manifest also contains a package section that will be converted
    /// as well.
    ///
    /// The `root_directory` is used to resolve relative paths, if it is `None`,
    /// paths are not checked.
    pub fn into_package_manifest(
        self,
        external: WorkspacePackageProperties,
        package_defaults: PackageDefaults,
        workspace: &WorkspaceManifest,
        root_directory: &Path,
    ) -> Result<(PackageManifest, Vec<Warning>), TomlError> {
        let Some(PixiSpanned {
            value: package,
            span: package_span,
        }) = self.package
        else {
            return Err(TomlError::MissingField("package".into(), None));
        };

        if !workspace
            .workspace
            .preview
            .is_enabled(KnownPreviewFeature::PixiBuild)
        {
            return Err(FeatureNotEnabled::new(
                format!(
                    "[package] section is only allowed when the `{}` feature is enabled",
                    KnownPreviewFeature::PixiBuild
                ),
                KnownPreviewFeature::PixiBuild,
            )
            .with_opt_span(package_span)
            .into());
        }

        let WithWarnings {
            value: package,
            warnings,
        } = package.into_manifest(
            external,
            package_defaults,
            workspace.preview(),
            root_directory,
        )?;
        Ok((package, warnings))
    }

    /// Assume that the manifest is a workspace manifest and convert it as such.
    ///
    /// If the manifest also contains a package section that will be converted
    /// as well.
    ///
    /// The `root_directory` is used to resolve relative paths, if it is `None`,
    /// paths are not checked.
    pub fn into_workspace_manifest(
        self,
        mut external: ExternalWorkspaceProperties,
        package_defaults: PackageDefaults,
        root_directory: &Path,
    ) -> Result<(WorkspaceManifest, Option<PackageManifest>, Vec<Warning>), TomlError> {
        let workspace = self
            .workspace
            .ok_or_else(|| TomlError::MissingField("project/workspace".into(), None))?;

        let preview = &workspace.value.preview;
        let pixi_build_enabled = preview.is_enabled(KnownPreviewFeature::PixiBuild);

        // Inline package definitions declared on dependencies are converted into
        // full package manifests while building the targets below, so they must
        // inherit the same workspace package properties an on-disk `[package]`
        // would. Assemble those properties from the workspace's external
        // properties up front; the workspace manifest itself is not built yet.
        let inline_workspace_properties = WorkspacePackageProperties {
            name: external.name.clone(),
            version: external.version.clone(),
            description: external.description.clone(),
            authors: external.authors.clone(),
            license: external.license.clone(),
            license_file: external.license_file.clone(),
            readme: external.readme.clone(),
            homepage: external.homepage.clone(),
            repository: external.repository.clone(),
            documentation: external.documentation.clone(),
            dependencies: IndexMap::new(),
            workspace_root: Some(root_directory.to_path_buf()),
        };

        let WithWarnings {
            value: default_workspace_target,
            mut warnings,
        } = TomlTarget {
            dependencies: self.dependencies,
            host_dependencies: self.host_dependencies,
            build_dependencies: self.build_dependencies,
            constraints: self.constraints,
            pypi_dependencies: self.pypi_dependencies.map(PixiSpanned::into_inner),
            dev_dependencies: self.dev_dependencies.map(PixiSpanned::into_inner),
            activation: self.activation.map(PixiSpanned::into_inner),
            tasks: self.tasks.map(PixiSpanned::into_inner).unwrap_or_default(),
            warnings: self.warnings,
        }
        .into_workspace_target(
            None,
            preview,
            &inline_workspace_properties,
            root_directory,
        )?;

        let known_platforms = &workspace.value.platforms.value;

        let mut workspace_targets = IndexMap::new();
        for (selector, target) in self.target.map(|t| t.value).unwrap_or_default() {
            // Verify that the target selector matches at least one of the platforms of the
            // workspace.
            let matching_platforms = known_platforms
                .iter()
                .filter(|p| selector.value.matches(p))
                .collect::<Vec<_>>();

            if matching_platforms.is_empty() {
                let warning = create_unsupported_selector_warning(
                    PlatformSpan::Workspace(workspace.value.platforms.span),
                    &selector,
                    &matching_platforms,
                );
                warnings.push(warning.into());
            }

            let WithWarnings {
                value: workspace_target,
                warnings: mut target_warnings,
            } = target.into_workspace_target(
                Some(selector.value.clone()),
                preview,
                &inline_workspace_properties,
                root_directory,
            )?;
            workspace_targets.insert(selector, workspace_target);
            warnings.append(&mut target_warnings);
        }

        // Sysreqs flow through this local map for the per-feature compatibility
        // check and the `[system-requirements]` migration; nothing else keeps
        // hold of them.
        if let Some(system_requirements) = &self.system_requirements
            && !system_requirements.value.is_empty()
        {
            warnings
                .push(Deprecation::system_requirements(system_requirements.span.clone()).into());
        }
        let default_sysreqs = self
            .system_requirements
            .map(PixiSpanned::into_inner)
            .unwrap_or_default();
        let mut feature_sysreqs: HashMap<FeatureName, SystemRequirements> =
            HashMap::from([(FeatureName::default(), default_sysreqs)]);

        // Construct a default feature
        let default_feature = Feature {
            name: FeatureName::default(),

            // The default feature does not overwrite the platforms or channels from the project
            // metadata.
            platforms: None,
            channels: None,

            channel_priority: workspace.value.channel_priority,
            solve_strategy: workspace.value.solve_strategy,

            // Use the pypi-options from the manifest for
            // the default feature
            pypi_options: self.pypi_options.map(PixiSpanned::into_inner),

            // Combine the default target with all user specified targets
            targets: Targets::from_default_and_user_defined(
                default_workspace_target,
                workspace_targets,
            ),
        };

        // Construct the features including the default feature
        let mut feature_name_to_span = IndexMap::new();
        let features: IndexMap<FeatureName, Feature> =
            IndexMap::from_iter([(FeatureName::default(), default_feature)]);
        let named_features = self
            .feature
            .map(PixiSpanned::into_inner)
            .unwrap_or_default()
            .into_iter()
            .map(|(name, feature)| {
                if name.value.is_default() {
                    return Err(TomlError::from(
                        GenericError::new("The feature 'default' is reserved and cannot be redefined")
                            .with_opt_span(name.span)
                            .with_help("All tables at the root of the document are implicitly added to the 'default' feature, use those instead."),
                    ));
                }
                let WithWarnings {
                    value: (feature, sysreqs),
                    warnings: mut feature_warnings,
                } = feature.into_feature(
                    name.value.clone(),
                    preview,
                    &workspace.value,
                    &inline_workspace_properties,
                    root_directory,
                )?;
                warnings.append(&mut feature_warnings);
                feature_sysreqs.insert(name.value.clone(), sysreqs);
                feature_name_to_span
                    .entry(name.value.clone())
                    .or_insert(name.span);
                Ok((name.value, feature))
            })
            .collect::<Result<IndexMap<FeatureName, Feature>, TomlError>>()?;
        let mut features = features
            .into_iter()
            .chain(named_features)
            .collect::<IndexMap<_, _>>();

        // Add external features if they are not overwritten
        let external_features = std::mem::take(&mut external.features);
        for (feature_name, feature) in external_features {
            if !features.contains_key(&feature_name) {
                features.insert(feature_name, feature);
            }
        }

        // Construct the environments including the default environment
        let mut environments = Environments::default();
        let mut solve_groups = SolveGroups::default();

        // Add the default environment first if it was not redefined.
        let toml_environments = self
            .environments
            .map(PixiSpanned::into_inner)
            .unwrap_or_default();
        if !toml_environments.contains_key(&EnvironmentName::Default) {
            environments.environments.push(Some(Environment::default()));
            environments
                .by_name
                .insert(EnvironmentName::Default, EnvironmentIdx(0));
        }

        // Add all named environments
        let mut features_used_by_environments = HashSet::new();
        for (name, env) in toml_environments {
            // Decompose the TOML
            let (inline, included_features, features_span, solve_group, no_default_feature) =
                match env {
                    TomlEnvironmentList::Map(env) => {
                        let TomlEnvironment {
                            features,
                            solve_group,
                            no_default_feature,
                            inline,
                        } = *env;
                        let (features, features_span) = features.map_or_else(
                            || (Vec::new(), None),
                            |Spanned { value, span }| (value, Some(span)),
                        );
                        (
                            Some(inline),
                            features,
                            features_span,
                            solve_group,
                            no_default_feature,
                        )
                    }
                    TomlEnvironmentList::Seq(features) => {
                        (None, features.value, Some(features.span), None, false)
                    }
                };

            // Synthesize the implicit feature that carries the environment's
            // inline content and prepend it to the environment's features.
            let inline_feature_name = match inline {
                Some(inline) if !inline.is_empty() => {
                    if name.is_default() {
                        return Err(TomlError::from(
                            GenericError::new(
                                "The 'default' environment cannot define dependencies inline",
                            )
                            .with_help(
                                "Add these dependencies to the top-level tables (for example '[dependencies]'); they are part of the default environment.",
                            ),
                        ));
                    }
                    let feature_name = FeatureName::environment(&name);
                    let WithWarnings {
                        value: feature,
                        warnings: mut feature_warnings,
                    } = inline.into_feature(
                        feature_name.clone(),
                        preview,
                        &workspace.value,
                        &inline_workspace_properties,
                        root_directory,
                    )?;
                    warnings.append(&mut feature_warnings);
                    features.insert(feature_name.clone(), feature);
                    Some(feature_name)
                }
                _ => None,
            };
            let included_features: Vec<Spanned<FeatureName>> = included_features
                .into_iter()
                .map(|Spanned { value, span }| Spanned {
                    value: FeatureName::from(value),
                    span,
                })
                .collect();

            features_used_by_environments
                .extend(included_features.iter().map(|span| span.value.clone()));

            // Verify that the features of the environment actually exist and that they are
            // not defined twice.
            let mut features_seen_where = HashMap::new();
            let mut used_features = Vec::with_capacity(included_features.len() + 1);
            if let Some(feature_name) = &inline_feature_name {
                used_features.push(
                    features
                        .get(feature_name)
                        .expect("the inline feature was just inserted"),
                );
            }
            for Spanned {
                value: feature_name,
                span,
            } in &included_features
            {
                if feature_name.is_environment()
                    || feature_name
                        .as_str()
                        .starts_with(consts::ENVIRONMENT_FEATURE_PREFIX)
                {
                    return Err(TomlError::from(
                        GenericError::new(format!(
                            "The feature '{feature_name}' cannot be referenced: names starting with '{}' refer to content defined inline on an environment",
                            consts::ENVIRONMENT_FEATURE_PREFIX,
                        ))
                        .with_span((*span).into())
                        .with_help("Content defined inline on an environment is private to that environment. Define a named feature to share content between environments."),
                    ));
                }
                let Some(feature) = features.get(feature_name) else {
                    return Err(TomlError::from(
                        GenericError::new(format!(
                            "The feature '{feature_name}' is not defined in the manifest",
                        ))
                        .with_span((*span).into())
                        .with_help("Add the feature to the manifest"),
                    ));
                };

                if let Some(previous_span) = features_seen_where.insert(feature_name, *span) {
                    return Err(TomlError::from(
                        GenericError::new(format!("The feature '{}' is included more than once.", feature.name))
                            .with_span((*span).into())
                            .with_span_label("the feature is included here")
                            .with_help("Since the order of the features matters, a duplicate feature is ambiguous")
                            .with_label(LabeledSpan::new_with_span(Some(String::from("the feature was previously included here")),
                                                                   Range::<usize>::from(previous_span)))));
                }

                used_features.push(feature);
            }

            // Choose whether to include the default
            if !no_default_feature {
                used_features.push(
                    features
                        .get(&FeatureName::Default)
                        .expect("default feature must exist"),
                );
            };

            // Ensure that the system requirements of all the features are compatible
            if let Err(e) = used_features
                .iter()
                .filter_map(|feature| feature_sysreqs.get(&feature.name))
                .try_fold(SystemRequirements::default(), |acc, req| acc.union(req))
            {
                return Err(TomlError::from(
                    GenericError::new(e.to_string())
                        .with_opt_span(features_span.map(Into::into))
                        .with_span_label(
                            "while resolving system requirements of features defined here",
                        ),
                ));
            }

            // Check that there are no conflicts in pypi options between features.
            // The workspace-level pypi-options act as an always-applied base
            // and are intentionally allowed to be overridden by feature-level
            // options, so they are not part of this conflict check; we only
            // verify that the features included in this environment do not
            // disagree with each other on any single-assignment field.
            if let Err(err) = used_features
                .iter()
                .filter_map(|feature| feature.pypi_options())
                .try_fold(PypiOptions::default(), |acc, opts| acc.union(opts))
            {
                return Err(TomlError::from(
                    GenericError::new(err.to_string())
                        .with_opt_span(features_span.map(Into::into))
                        .with_span_label("while resolving pypi options of features defined here"),
                ));
            }

            let mut feature_names: Vec<FeatureName> =
                included_features.into_iter().map(Spanned::take).collect();
            if let Some(feature_name) = inline_feature_name {
                feature_names.insert(0, feature_name);
            }

            let environment_idx = EnvironmentIdx(environments.environments.len());
            environments.by_name.insert(name.clone(), environment_idx);
            environments.environments.push(Some(Environment {
                name,
                features: feature_names,
                solve_group: solve_group.map(|sg| solve_groups.add(sg, environment_idx)),
                no_default_feature,
            }));
        }

        // Verify that all features are used in at least one environment
        for (feature_name, span) in feature_name_to_span {
            if features_used_by_environments.contains(&feature_name) {
                continue;
            }

            warnings.push(Warning::from(
                GenericError::new(format!(
                    "The feature '{feature_name}' is defined but not used in any environment. Dependencies of unused features are not resolved or checked, and use wildcard (*) version specifiers by default, disregarding any set `pinning-strategy`"
                ))
                .with_opt_span(span)
                .with_help("Remove the feature from the manifest or add it to an environment"),
            ));
        }

        // Get the name from the [package] section if it's missing from the workspace.
        let project_name = self
            .package
            .as_ref()
            .and_then(|p| p.value.name.as_ref())
            .and_then(|field| field.clone().value());

        let WithWarnings {
            warnings: mut workspace_warnings,
            value: mut workspace,
        } = workspace.value.into_workspace(
            ExternalWorkspaceProperties {
                name: project_name.or(external.name),
                ..external
            },
            root_directory,
        )?;
        warnings.append(&mut workspace_warnings);
        workspace.exclude_newer_package_overrides = self
            .exclude_newer
            .map(PixiSpanned::into_inner)
            .unwrap_or_default();
        workspace.pypi_exclude_newer_package_overrides = self
            .pypi_exclude_newer
            .map(PixiSpanned::into_inner)
            .unwrap_or_default();

        migrate_system_requirements_to_platforms(&mut workspace, &mut features, &feature_sysreqs)?;

        let mut workspace_manifest = WorkspaceManifest {
            workspace,
            features,
            environments,
            solve_groups,
        };
        workspace_manifest.register_composed_platforms()?;

        let package_manifest = if let Some(PixiSpanned {
            value: package,
            span: package_span,
        }) = self.package
        {
            if !pixi_build_enabled {
                return Err(FeatureNotEnabled::new(
                    format!(
                        "[package] section is only allowed when the `{}` feature is enabled",
                        KnownPreviewFeature::PixiBuild
                    ),
                    KnownPreviewFeature::PixiBuild,
                )
                .with_opt_span(package_span)
                .into());
            }

            let WithWarnings {
                value: package_manifest,
                warnings: mut package_warnings,
            } = package.into_manifest(
                workspace_manifest.workspace_package_properties(),
                package_defaults,
                &workspace_manifest.workspace.preview,
                root_directory,
            )?;
            warnings.append(&mut package_warnings);

            Some(package_manifest)
        } else {
            None
        };

        Ok((workspace_manifest, package_manifest, warnings))
    }
}

/// Rebuild `workspace.platforms` so it owns one `PixiPlatform` per
/// `(subdir, virtual_packages)` shape any feature actually references, and
/// rewrite each feature's platforms list to name those entries instead of the
/// originals + `[system-requirements]`. The original manifest list is kept in
/// a local for the duration of this call and is what the serializer round-trips
/// back to pixi.toml.
fn migrate_system_requirements_to_platforms(
    workspace: &mut crate::Workspace,
    features: &mut IndexMap<FeatureName, Feature>,
    feature_sysreqs: &HashMap<FeatureName, SystemRequirements>,
) -> Result<(), TomlError> {
    // A workspace that already declares custom names or per-platform virtual
    // packages can't accept the legacy `[system-requirements]` shim -- it
    // would be ambiguous which set of declarations wins.
    let all_simple_subdir = workspace
        .platforms
        .iter()
        .all(PixiPlatform::is_subdir_platform);

    let has_any_sysreqs = feature_sysreqs.values().any(|s| !s.is_empty());
    // Any feature (incl. default) carries `[system-requirements]` and the
    // workspace is still in legacy subdir-only shape: a follow-up add/edit
    // of a non-subdir platform will commit the migration to pixi.toml.
    workspace.must_migrate = all_simple_subdir && has_any_sysreqs;

    // Subdir-only declarations let environments combine the per-feature rich
    // platforms by subdir; custom rich platforms are matched by name.
    workspace.use_platform_composition = all_simple_subdir;

    if all_simple_subdir {
        extend_originals_with_referenced_subdirs(&mut workspace.platforms, features)?;
    }

    // Without `[system-requirements]` there is nothing to migrate: keep every
    // declared platform exactly as written -- including two distinct platforms
    // that share a subdir, e.g. `linux-64` plus `linux-64-cuda-12-9` -- and
    // only check that each feature reference resolves.
    if !has_any_sysreqs {
        for feature in features.values() {
            validate_referenced_platforms(&workspace.platforms, feature)?;
        }
        return Ok(());
    }

    // Legacy migration: rebuild the platform list so each bare subdir is
    // replaced by the virtual-package-bearing variant its system requirements
    // imply, and rewrite each feature's platforms to name those entries.
    let originals: IndexSet<PixiPlatform> = std::mem::take(&mut workspace.platforms);
    for feature in features.values_mut() {
        let sysreqs = feature_sysreqs.get(&feature.name);
        if sysreqs.is_none_or(SystemRequirements::is_empty) {
            register_referenced_originals(&originals, feature, &mut workspace.platforms)?;
            continue;
        }
        if !all_simple_subdir {
            return Err(TomlError::from(GenericError::new(format!(
                "feature '{}' uses `[system-requirements]` but the workspace declares per-platform virtual packages; remove the system-requirements table and declare the constraints on the platforms instead",
                feature.name,
            ))));
        }
        let sysreqs = sysreqs.expect("checked just above");
        synthesise_for_feature(&originals, feature, sysreqs, &mut workspace.platforms)?;
    }

    append_uncovered_subdirs(&originals, &mut workspace.platforms);
    Ok(())
}

/// Pre-scan pass for the simple-subdir-only workspace case: every name in
/// any feature's platforms list that isn't already declared in the workspace
/// is appended to `originals` as a bare subdir-platform, provided the name
/// parses as a conda subdir. Names that don't parse are a hard error.
fn extend_originals_with_referenced_subdirs(
    originals: &mut IndexSet<PixiPlatform>,
    features: &IndexMap<FeatureName, Feature>,
) -> Result<(), TomlError> {
    for feature in features.values() {
        let Some(names) = feature.platforms.as_ref() else {
            continue;
        };
        for name in names {
            if originals.iter().any(|p| p.name() == name) {
                continue;
            }
            let subdir = Platform::from_str(name.as_str()).map_err(|e| {
                TomlError::from(GenericError::new(format!(
                    "feature '{}' references platform '{}' which is neither declared in the workspace nor a valid conda subdir: {e}",
                    feature.name, name,
                )))
            })?;
            originals.insert(PixiPlatform::from_subdir(subdir));
        }
    }
    Ok(())
}

/// Error if any name in `feature.platforms` does not resolve to a platform
/// declared in the workspace.
fn validate_referenced_platforms(
    platforms: &IndexSet<PixiPlatform>,
    feature: &Feature,
) -> Result<(), TomlError> {
    let Some(names) = feature.platforms.as_ref() else {
        return Ok(());
    };
    for name in names {
        if !platforms.iter().any(|p| p.name() == name) {
            return Err(TomlError::from(GenericError::new(format!(
                "feature '{}' references platform '{}' which is not declared in the workspace",
                feature.name, name,
            ))));
        }
    }
    Ok(())
}

/// Resolve every name in `feature.platforms` to an original `PixiPlatform`
/// and copy it into `workspace.platforms`. Error if a name is missing.
fn register_referenced_originals(
    originals: &IndexSet<PixiPlatform>,
    feature: &Feature,
    target: &mut IndexSet<PixiPlatform>,
) -> Result<(), TomlError> {
    let Some(names) = feature.platforms.as_ref() else {
        return Ok(());
    };
    for name in names {
        let original = originals.iter().find(|p| p.name() == name).ok_or_else(|| {
            TomlError::from(GenericError::new(format!(
                "feature '{}' references platform '{}' which is not declared in the workspace",
                feature.name, name,
            )))
        })?;
        target.insert(original.clone());
    }
    Ok(())
}

/// For a feature that carries `[system-requirements]`, synthesise one
/// `PixiPlatform` per subdir the feature targets, register it in
/// `workspace.platforms`, and rewrite the feature's platforms list to those
/// synthetic names (the default feature keeps `platforms = None`).
fn synthesise_for_feature(
    originals: &IndexSet<PixiPlatform>,
    feature: &mut Feature,
    sysreqs: &SystemRequirements,
    target: &mut IndexSet<PixiPlatform>,
) -> Result<(), TomlError> {
    let subdirs: Vec<Platform> = match feature.platforms.as_ref() {
        Some(names) => names
            .iter()
            .map(|name| {
                Platform::from_str(name.as_str()).map_err(|e| {
                    TomlError::from(GenericError::new(format!(
                        "feature '{}' references platform '{}' which is not a conda subdir: {e}",
                        feature.name, name,
                    )))
                })
            })
            .collect::<Result<_, _>>()?,
        None => originals.iter().map(PixiPlatform::subdir).collect(),
    };

    let candidates = sysreqs.to_declared_virtual_packages();
    let mut synthesised_names: IndexSet<PixiPlatformName> = IndexSet::new();
    for subdir in subdirs {
        let declared: Vec<GenericVirtualPackage> = candidates
            .iter()
            .filter(|c| {
                crate::system_requirements::virtual_package_applies_to_subdir(
                    c.name.as_normalized(),
                    subdir,
                )
            })
            .cloned()
            .collect();
        let mut name_str = crate::toml::platform::synthesize_name_string(subdir, &declared);
        // A sysreq that matches the subdir defaults collapses the name to the
        // bare subdir, a reserved name a platform entry can't carry. Keep the
        // declaration under a distinct `-generic` name instead of failing the
        // subdir-platform invariant.
        if !declared.is_empty() && name_str == subdir.as_str() {
            name_str = format!("{name_str}-generic");
        }
        let name = PixiPlatformName::try_from(name_str.as_str()).map_err(|e| {
            TomlError::from(GenericError::new(format!(
                "synthesised platform name '{name_str}' is not a valid pixi platform name: {e}",
            )))
        })?;
        target.insert(
            PixiPlatform::new_with_defaults(name.clone(), subdir, declared).map_err(|e| {
                TomlError::from(GenericError::new(format!(
                    "synthesised platform '{name}' is invalid: {e}",
                )))
            })?,
        );
        synthesised_names.insert(name);
    }

    // Point every feature, the default included, at its synthesised platforms
    // so environments can read the virtual packages back off them when composing.
    feature.platforms = Some(synthesised_names);
    Ok(())
}

/// Append any original platform whose subdir isn't already represented in
/// `target`. Subdir coverage is snapshotted before the walk -- entries
/// appended during the walk do not affect later iterations.
fn append_uncovered_subdirs(
    originals: &IndexSet<PixiPlatform>,
    target: &mut IndexSet<PixiPlatform>,
) {
    let covered: HashSet<Platform> = target.iter().map(PixiPlatform::subdir).collect();
    for original in originals {
        if !covered.contains(&original.subdir()) {
            target.insert(original.clone());
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlManifest {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let mut warnings = Vec::new();

        let workspace = if th.contains("workspace") {
            Some(th.required_s("workspace")?.into())
        } else {
            let project: Option<Spanned<TomlWorkspace>> = th.optional("project");
            if let Some(project) = &project {
                warnings
                    .push(Deprecation::renamed_field("project", "workspace", project.span).into());
            }
            project.map(From::from)
        };
        let package = th.optional("package");

        let target = th
            .optional::<TomlWith<_, PixiSpanned<TomlIndexMap<PixiSpanned<TargetSelector>, Same>>>>(
                "target",
            )
            .map(TomlWith::into_inner);

        let dependencies = th.optional("dependencies");

        let host_dependencies: Option<Spanned<DependencyTable>> = th.optional("host-dependencies");
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

        let build_dependencies: Option<Spanned<DependencyTable>> =
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
        let exclude_newer = th
            .optional::<PixiSpanned<TomlIndexMap<PackageName, TomlFromStr<ExcludeNewer>>>>(
                "exclude-newer",
            )
            .map(|map| PixiSpanned {
                span: map.span,
                value: map
                    .value
                    .into_inner()
                    .into_iter()
                    .map(|(name, cutoff): (PackageName, TomlFromStr<ExcludeNewer>)| {
                        (name, cutoff.into_inner())
                    })
                    .collect::<IndexMap<_, _>>(),
            });

        let pypi_dependencies = th
            .optional::<TomlWith<_, PixiSpanned<TomlIndexMap<_, Same>>>>("pypi-dependencies")
            .map(TomlWith::into_inner);
        let pypi_exclude_newer = th
            .optional::<PixiSpanned<TomlIndexMap<PypiPackageName, TomlFromStr<ExcludeNewer>>>>(
                "pypi-exclude-newer",
            )
            .map(|map| PixiSpanned {
                span: map.span,
                value: map
                    .value
                    .into_inner()
                    .into_iter()
                    .map(
                        |(name, cutoff): (PypiPackageName, TomlFromStr<ExcludeNewer>)| {
                            (name, cutoff.into_inner())
                        },
                    )
                    .collect::<IndexMap<_, _>>(),
            });
        let dev = th
            .optional::<TomlWith<_, PixiSpanned<TomlIndexMap<_, Same>>>>("dev")
            .map(TomlWith::into_inner);
        let activation = th.optional("activation");
        let tasks = th
            .optional::<TomlWith<_, PixiSpanned<TomlHashMap<_, Same>>>>("tasks")
            .map(|with| {
                let inner: PixiSpanned<HashMap<String, TomlTask>> = with.into_inner();
                PixiSpanned {
                    value: inner
                        .value
                        .into_iter()
                        .map(|(key, value)| {
                            let WithWarnings {
                                value: task,
                                warnings: mut task_warnings,
                            } = value;
                            warnings.append(&mut task_warnings);
                            (key.into(), task)
                        })
                        .collect(),
                    span: inner.span,
                }
            });
        let feature = th
            .optional::<TomlWith<_, PixiSpanned<TomlIndexMap<_, Same>>>>("feature")
            .map(TomlWith::into_inner);
        let environments = th
            .optional::<TomlWith<_, PixiSpanned<TomlIndexMap<_, Same>>>>("environments")
            .map(TomlWith::into_inner);
        let pypi_options = th.optional("pypi-options");
        let system_requirements = th.optional("system-requirements");

        // Parse the tool section by ignoring it.
        if let Some(mut tool) = th.table.remove("tool") {
            match tool.take() {
                ValueInner::Table(_) => {}
                other => {
                    return Err(expected("a table", other, tool.span).into());
                }
            }
        }

        // Parse the $schema section by ignoring it.
        if let Some(mut schema) = th.table.remove("$schema") {
            match schema.take() {
                ValueInner::String(_) => {}
                other => {
                    return Err(expected("a string", other, schema.span).into());
                }
            }
        }

        th.finalize(None)?;

        Ok(TomlManifest {
            workspace,
            package,
            system_requirements,
            target,
            dependencies,
            host_dependencies,
            build_dependencies,
            constraints,
            exclude_newer,
            pypi_dependencies,
            pypi_exclude_newer,
            dev_dependencies: dev,
            activation,
            tasks,
            feature,
            environments,
            pypi_options,
            warnings,
        })
    }
}

/// Defines some of the properties that might be defined in other parts of the
/// manifest but we do require to be set in the workspace section.
///
/// This can be used to inject these properties.
#[derive(Debug, Clone, Default)]
pub struct ExternalWorkspaceProperties {
    pub name: Option<String>,
    pub version: Option<Version>,
    pub description: Option<String>,
    pub authors: Option<Vec<String>>,
    pub license: Option<String>,
    pub license_file: Option<PathBuf>,
    pub readme: Option<PathBuf>,
    pub homepage: Option<Url>,
    pub repository: Option<Url>,
    pub documentation: Option<Url>,
    pub features: IndexMap<FeatureName, Feature>,
}

#[cfg(test)]
mod test {
    use insta::assert_snapshot;
    use pixi_test_utils::format_parse_error;
    use rattler_conda_types::Platform;

    use super::*;
    use crate::{
        InlineContentHash, PixiPlatform, toml::FromTomlStr,
        utils::test_utils::expect_parse_warnings,
    };

    /// A helper function that generates a snapshot of the error message when
    /// parsing a manifest TOML. The error is returned.
    #[must_use]
    pub(crate) fn expect_parse_failure(pixi_toml: &str) -> String {
        let parse_error = <TomlManifest as FromTomlStr>::from_toml_str(pixi_toml)
            .and_then(|manifest| {
                manifest.into_workspace_manifest(
                    ExternalWorkspaceProperties::default(),
                    PackageDefaults::default(),
                    Path::new(""),
                )
            })
            .expect_err("parsing should fail");

        format_parse_error(pixi_toml, parse_error)
    }

    #[test]
    fn test_package_without_build_section() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []

        [package]

        [package.build]
        backend = { name = "foobar", version = "*" }
        "#,
        ));
    }

    #[test]
    fn test_system_requirements_migration_replaces_originals_with_synthetics() {
        let workspace_manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
            [workspace]
            name = "test"
            channels = []
            platforms = ["linux-64", "osx-64", "win-64"]

            [system-requirements]
            linux = "5.10"
            libc = "2.28"
            cuda = "12.0"
            "#,
            Path::new(""),
        )
        .unwrap();

        let names: Vec<&str> = workspace_manifest
            .workspace
            .platforms
            .iter()
            .map(|p| p.name().as_str())
            .collect();
        assert_eq!(names.len(), 3, "got {names:?}");
        // linux/libc are linux-only and __cuda is filtered off osx, so the
        // osx-64 entry stays bare while linux-64 / win-64 get cuda-bearing names.
        for synthetic in ["linux-64", "win-64"] {
            assert!(
                names
                    .iter()
                    .any(|n| n.starts_with(synthetic) && n.contains("cuda-12-0")),
                "expected a cuda-bearing synthetic for {synthetic}, got {names:?}",
            );
            assert!(
                !names.contains(&synthetic),
                "bare {synthetic} should be gone after migration, got {names:?}",
            );
        }
        assert!(
            names.contains(&"osx-64"),
            "osx-64 must stay bare since no sysreqs apply there, got {names:?}",
        );
        // The default feature carries the workspace `[system-requirements]`, so
        // it now points at the synthesised platforms rather than `None`.
        let default = workspace_manifest
            .features
            .get(&FeatureName::Default)
            .unwrap();
        let mut default_platforms: Vec<&str> = default
            .platforms
            .as_ref()
            .expect("default feature carries the migrated platforms")
            .iter()
            .map(|name| name.as_str())
            .collect();
        let mut sorted_names = names.clone();
        default_platforms.sort_unstable();
        sorted_names.sort_unstable();
        assert_eq!(default_platforms, sorted_names);
    }

    /// A legacy sysreq that exactly matches the subdir defaults (glibc on
    /// linux-64) collapses the synthesised name to the bare subdir, a reserved
    /// name a platform entry can't carry. The migration must fall back to
    /// `-generic` rather than failing with `IsSubdirPlatform`. Regression for
    /// a real `pixi install` failure on such manifests.
    #[test]
    fn test_system_requirements_migration_default_matching_sysreq_uses_generic_name() {
        let glibc = pixi_default_versions::default_glibc_version();
        let workspace_manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            format!(
                r#"
            [workspace]
            name = "test"
            channels = []
            platforms = ["linux-64"]

            [system-requirements]
            libc = "{glibc}"
            "#
            ),
            Path::new(""),
        )
        .unwrap();

        let names: Vec<&str> = workspace_manifest
            .workspace
            .platforms
            .iter()
            .map(|p| p.name().as_str())
            .collect();
        assert_eq!(names, vec!["linux-64-generic"], "got {names:?}");
    }

    #[test]
    fn test_system_requirements_migration_named_feature_rewrites_platforms() {
        let workspace_manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
            [workspace]
            name = "test"
            channels = []
            platforms = ["linux-64", "osx-64"]

            [feature.cuda]
            platforms = ["linux-64"]
            system-requirements = { cuda = "12.0" }

            [environments]
            cuda = ["cuda"]
            "#,
            Path::new(""),
        )
        .unwrap();

        let names: HashSet<&str> = workspace_manifest
            .workspace
            .platforms
            .iter()
            .map(|p| p.name().as_str())
            .collect();
        // `linux-64-cuda-12-0` from feature.cuda, the bare `osx-64` appended
        // because no feature covers its subdir, and a bare `linux-64` the
        // cuda-free default environment composes for that subdir.
        assert_eq!(
            names,
            HashSet::from(["linux-64-cuda-12-0", "osx-64", "linux-64"]),
        );

        let cuda = workspace_manifest
            .features
            .get(&FeatureName::from("cuda"))
            .unwrap();
        assert_eq!(
            cuda.platforms
                .as_ref()
                .unwrap()
                .iter()
                .next()
                .unwrap()
                .as_str(),
            "linux-64-cuda-12-0",
        );
    }

    #[test]
    fn test_system_requirements_migration_rejects_rich_workspace_platform() {
        let result = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
            [workspace]
            name = "test"
            channels = []
            platforms = [
              "linux-64",
              { name = "gpu", platform = "linux-64", cuda = "12.0" },
            ]

            [system-requirements]
            cuda = "11.0"
            "#,
            Path::new(""),
        );
        let err = result.expect_err("rich platform + sysreqs must error");
        assert!(
            err.error
                .to_string()
                .contains("per-platform virtual packages"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn test_system_requirements_migration_rejects_rich_workspace_platform_same_name() {
        // Regression: rich `name == platform` entries used to be silently
        // demoted to bare subdirs and lose their VPs to [system-requirements].
        let result = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
            [workspace]
            name = "test"
            channels = []
            platforms = [{ name = "osx-arm64", platform = "osx-arm64", macos = "14" }]

            [system-requirements]
            macos = "13.3"
            "#,
            Path::new(""),
        );
        let err = result.expect_err("rich platform + sysreqs must error");
        let msg = err.error.to_string();
        assert!(
            msg.contains("special subdir platform"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn test_system_requirements_migration_no_sysreqs_passes_through() {
        let workspace_manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
            [workspace]
            name = "test"
            channels = []
            platforms = ["linux-64", "osx-64"]

            [feature.dev]
            platforms = ["linux-64"]
            "#,
            Path::new(""),
        )
        .unwrap();

        let names: Vec<&str> = workspace_manifest
            .workspace
            .platforms
            .iter()
            .map(|p| p.name().as_str())
            .collect();
        // No sysreqs anywhere: workspace.platforms ends up matching originals.
        assert_eq!(names, vec!["linux-64", "osx-64"]);

        let dev = workspace_manifest
            .features
            .get(&FeatureName::from("dev"))
            .unwrap();
        assert_eq!(
            dev.platforms
                .as_ref()
                .unwrap()
                .iter()
                .next()
                .unwrap()
                .as_str(),
            "linux-64",
        );
    }

    #[test]
    fn test_rich_workspace_feature_references_undeclared_platform_errors() {
        // No system-requirements, so `extend_originals_with_referenced_subdirs`
        // doesn't run for this rich-platform workspace; a feature naming a
        // platform the workspace never declares must still be rejected.
        let result = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
            [workspace]
            name = "test"
            channels = []
            platforms = ["linux-64", { platform = "linux-64", cuda = "12.9" }]

            [feature.gpu]
            platforms = ["linux-64-cuda-13-0"]
            "#,
            Path::new(""),
        );
        let err = result.expect_err("undeclared feature platform must error");
        assert!(
            err.error
                .to_string()
                .contains("references platform 'linux-64-cuda-13-0' which is not declared"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn test_workspace_name_from_package() {
        let workspace_manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
        [workspace]
        channels = []
        platforms = []
        preview = ["pixi-build"]

        [package]
        name = "foo"
        version = "0.1.0"

        [package.build]
        backend = { name = "foobar", version = "*" }
        "#,
            Path::new(""),
        )
        .unwrap();

        assert_eq!(workspace_manifest.workspace.name.as_deref(), Some("foo"));
    }

    #[test]
    fn test_run_dependencies_in_feature() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = []

        [feature.foobar.run-dependencies]
        "#,
        ));
    }

    #[test]
    fn test_source_spec_in_constraints() {
        // Path source specs are not allowed in [constraints]
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = []

        [constraints]
        my-package = { path = "../my-package" }
        "#,
        ));

        // Git source specs are not allowed in [constraints] either
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = []

        [feature.gpu.constraints]
        my-lib = { git = "https://github.com/example/my-lib" }
        "#,
        ));
    }

    #[test]
    fn test_host_dependencies_in_feature_with_pixi_build() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = []
        preview = ["pixi-build"]

        [package]

        [package.build]
        backend = { name = "foobar", version = "*" }

        [feature.foobar.host-dependencies]
        "#,
        ));
    }

    #[test]
    fn test_build_dependencies_in_feature_with_pixi_build() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = []
        preview = ["pixi-build"]

        [package]

        [package.build]
        backend = { name = "foobar", version = "*" }

        [feature.foobar.build-dependencies]
        "#,
        ));
    }

    #[test]
    fn test_invalid_non_package_sections() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = []
        preview = ["pixi-build"]

        [build-dependencies]

        [host-dependencies]

        [target.win.host-dependencies]
        "#,
        ));
    }

    #[test]
    fn test_tool_must_be_table() {
        assert_snapshot!(expect_parse_failure(
            r#"
        tool = false

        [workspace]
        channels = []
        platforms = []
        "#,
        ));
    }

    #[test]
    fn test_schema_must_be_string() {
        assert_snapshot!(expect_parse_failure(
            r#"
        schema = false

        [workspace]
        channels = []
        platforms = []
        "#,
        ));
    }

    /// Parses a workspace manifest and returns both the workspace and (required)
    /// package manifests, so package-side inheritance can be inspected.
    fn parse_workspace_and_package(source: &str) -> (WorkspaceManifest, PackageManifest) {
        let manifest = <TomlManifest as FromTomlStr>::from_toml_str(source).expect("parse toml");
        let (ws, pkg, _) = manifest
            .into_workspace_manifest(
                ExternalWorkspaceProperties::default(),
                PackageDefaults::default(),
                Path::new(""),
            )
            .expect("convert to workspace manifest");
        (ws, pkg.expect("package manifest"))
    }

    #[test]
    fn test_workspace_dependencies_pool_populates_workspace() {
        let (ws, _pkg) = parse_workspace_and_package(
            r#"
            [workspace]
            name = "monorepo"
            channels = []
            platforms = ['linux-64']
            preview = ["pixi-build"]

            [workspace.dependencies]
            numpy = "1.*"

            [package]
            name = "lib"
            version = "0.1.0"

            [package.build]
            backend = { name = "foobar", version = "*" }
            "#,
        );
        let numpy = ws
            .workspace
            .dependencies
            .get(&rattler_conda_types::PackageName::new_unchecked("numpy"))
            .expect("numpy in workspace pool");
        assert_eq!(numpy.version.as_ref().unwrap().to_string(), "1.*");
    }

    #[test]
    fn test_inline_package_definition_in_dependencies() {
        // An inline package definition on a source dependency is captured on the
        // target while the source spec stays in the regular dependency map.
        let manifest = <TomlManifest as FromTomlStr>::from_toml_str(
            r#"
            [workspace]
            channels = []
            platforms = ['linux-64']
            preview = ["pixi-build"]

            [dependencies]
            rust-package = { git = "https://github.com/user/repo.git", package.build = { backend = { name = "pixi-build-rust", version = "*" } } }
            "#,
        )
        .expect("parse toml");
        let (ws, _pkg, _warnings) = manifest
            .into_workspace_manifest(
                ExternalWorkspaceProperties::default(),
                PackageDefaults::default(),
                Path::new(""),
            )
            .expect("convert to workspace manifest");

        let default_target = ws.default_feature().targets.default();
        let name = rattler_conda_types::PackageName::new_unchecked("rust-package");

        // The source spec is retained as a normal dependency.
        let spec = default_target
            .run_dependencies()
            .and_then(|deps| deps.get(&name))
            .expect("source spec retained");
        assert!(spec.iter().all(|s| s.is_source()));

        // The inline package definition is captured on the target.
        let inline = default_target
            .inline_packages
            .get(&name)
            .expect("inline package stored")
            .manifest
            .clone();
        assert_eq!(inline.build.backend.name.as_normalized(), "pixi-build-rust");
        // The build source is taken from the dependency spec, not the inline body.
        assert!(inline.build.source.is_none());
    }

    /// Parse a manifest and return the content hash of the named inline package.
    fn inline_content_hash(manifest: &str, package: &str) -> InlineContentHash {
        let manifest = <TomlManifest as FromTomlStr>::from_toml_str(manifest).expect("parse toml");
        let (ws, _pkg, _warnings) = manifest
            .into_workspace_manifest(
                ExternalWorkspaceProperties::default(),
                PackageDefaults::default(),
                Path::new(""),
            )
            .expect("convert to workspace manifest");
        let name = rattler_conda_types::PackageName::new_unchecked(package);
        ws.default_feature()
            .targets
            .default()
            .inline_packages
            .get(&name)
            .expect("inline package stored")
            .content_hash
    }

    // A workspace declaring a single inline package `pkg`. The helpers below
    // perturb one aspect at a time to pin down what the content hash reacts to.
    const INLINE_HASH_MANIFEST: &str = r#"
        [workspace]
        channels = []
        platforms = ['linux-64']
        preview = ["pixi-build"]

        [dependencies]
        pkg = { git = "https://github.com/user/repo.git", package = { build = { backend = { name = "pixi-build-rust", version = "*" } } } }
        "#;

    #[test]
    fn test_inline_content_hash_is_deterministic() {
        // Parsing the same definition twice yields the same hash; nothing
        // run-dependent leaks into it.
        assert_eq!(
            inline_content_hash(INLINE_HASH_MANIFEST, "pkg"),
            inline_content_hash(INLINE_HASH_MANIFEST, "pkg"),
        );
    }

    #[test]
    fn test_inline_content_hash_ignores_formatting() {
        // The hash is taken over the assembled manifest, not the source text.
        // Dotted keys and a different field order parse to the same manifest, so
        // they must hash equally.
        let reformatted = r#"
            [workspace]
            channels = []
            platforms = ['linux-64']
            preview = ["pixi-build"]

            [dependencies]
            pkg = { git = "https://github.com/user/repo.git", package.build.backend = { version = "*", name = "pixi-build-rust" } }
            "#;
        assert_eq!(
            inline_content_hash(INLINE_HASH_MANIFEST, "pkg"),
            inline_content_hash(reformatted, "pkg"),
        );
    }

    #[test]
    fn test_inline_content_hash_changes_with_build_config() {
        // `build.config` is backend configuration that is not otherwise
        // represented in the build cache key, so editing it must change the
        // content hash. This is the property that makes hashing the whole
        // manifest (rather than just the dependencies) necessary.
        let with_config = r#"
            [workspace]
            channels = []
            platforms = ['linux-64']
            preview = ["pixi-build"]

            [dependencies]
            pkg = { git = "https://github.com/user/repo.git", package = { build = { backend = { name = "pixi-build-rust", version = "*" }, config = { extra = "value" } } } }
            "#;
        assert_ne!(
            inline_content_hash(INLINE_HASH_MANIFEST, "pkg"),
            inline_content_hash(with_config, "pkg"),
        );
    }

    #[test]
    fn test_inline_content_hash_folds_in_dependency_name() {
        // Two identical inline tables declared under different dependency names
        // get distinct hashes, so they never collide in the cache.
        let other_name = r#"
            [workspace]
            channels = []
            platforms = ['linux-64']
            preview = ["pixi-build"]

            [dependencies]
            other = { git = "https://github.com/user/repo.git", package = { build = { backend = { name = "pixi-build-rust", version = "*" } } } }
            "#;
        assert_ne!(
            inline_content_hash(INLINE_HASH_MANIFEST, "pkg"),
            inline_content_hash(other_name, "other"),
        );
    }

    /// Parse a `pyproject.toml` whose `[tool.pixi]` table declares an inline
    /// package and return that package's content hash.
    fn inline_content_hash_pyproject(manifest: &str, package: &str) -> InlineContentHash {
        let manifest =
            crate::pyproject::PyProjectManifest::from_toml_str(manifest).expect("parse pyproject");
        let (ws, _pkg, _warnings) = manifest
            .into_workspace_manifest(Path::new(""))
            .expect("convert pyproject to workspace manifest");
        let name = rattler_conda_types::PackageName::new_unchecked(package);
        ws.default_feature()
            .targets
            .default()
            .inline_packages
            .get(&name)
            .expect("inline package stored")
            .content_hash
    }

    #[test]
    fn test_inline_content_hash_is_consumer_format_independent() {
        // The same inline definition declared in a `pixi.toml` `[dependencies]`
        // entry and in a `pyproject.toml` `[tool.pixi.dependencies]` entry must
        // assemble to the same package manifest. The content hash folds in the
        // dependency name and package manifest but not the consuming workspace,
        // so the two formats must hash equally; otherwise the build cache key
        // would depend on which manifest format the consumer happened to use.
        let pixi_toml = r#"
            [workspace]
            channels = []
            platforms = ['linux-64']
            preview = ["pixi-build"]

            [dependencies]
            pkg = { path = "pkg", package = { build = { backend = { name = "pixi-build-python", version = "*" } }, run-dependencies = { rich = ">=13.9.4,<14" } } }
            "#;
        let pyproject = r#"
            [project]
            name = "consumer"
            version = "0.1.0"
            requires-python = ">=3.11"

            [tool.pixi.workspace]
            channels = []
            platforms = ['linux-64']
            preview = ["pixi-build"]

            [tool.pixi.dependencies]
            pkg = { path = "pkg", package = { build = { backend = { name = "pixi-build-python", version = "*" } }, run-dependencies = { rich = ">=13.9.4,<14" } } }
            "#;
        assert_eq!(
            inline_content_hash(pixi_toml, "pkg"),
            inline_content_hash_pyproject(pyproject, "pkg"),
        );
    }

    #[test]
    fn test_package_inherits_workspace_dependency() {
        // The host-dependencies entry uses { workspace = true } and the
        // workspace pool defines numpy. After parsing, the package's default
        // target must carry the inherited version spec.
        let (_ws, pkg) = parse_workspace_and_package(
            r#"
            [workspace]
            name = "monorepo"
            channels = []
            platforms = ['linux-64']
            preview = ["pixi-build"]

            [workspace.dependencies]
            numpy = "1.*"

            [package]
            name = "lib"
            version = "0.1.0"

            [package.build]
            backend = { name = "foobar", version = "*" }

            [package.host-dependencies]
            numpy = { workspace = true }
            "#,
        );
        let host_deps = pkg
            .dependencies
            .dependencies
            .get(&crate::SpecType::Host)
            .expect("host bucket");
        let numpy = host_deps
            .get(&rattler_conda_types::PackageName::new_unchecked("numpy"))
            .expect("numpy in host deps");
        let spec = numpy.iter().next().unwrap();
        assert_eq!(spec.as_version_spec().unwrap().to_string(), "1.*");
    }

    #[test]
    fn test_inherited_backend_pulls_version_from_workspace() {
        // The build backend uses workspace inheritance; the version must come
        // from `[workspace.dependencies]`.
        let (_ws, pkg) = parse_workspace_and_package(
            r#"
            [workspace]
            name = "monorepo"
            channels = []
            platforms = ['linux-64']
            preview = ["pixi-build"]

            [workspace.dependencies]
            pixi-build-python = "==1.2.3"

            [package]
            name = "lib"
            version = "0.1.0"

            [package.build]
            backend = { name = "pixi-build-python", workspace = true }
            "#,
        );
        let spec = &pkg.build.backend.spec;
        // The resolved backend spec must carry the workspace-declared version.
        match spec {
            pixi_spec::PixiSpec::DetailedVersion(detailed) => {
                assert_eq!(detailed.version.as_ref().unwrap().to_string(), "==1.2.3")
            }
            other => panic!("unexpected backend spec: {other:?}"),
        }
    }

    #[test]
    fn test_inherited_entry_missing_workspace_definition_errors() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "monorepo"
        channels = []
        platforms = ['linux-64']
        preview = ["pixi-build"]

        [package]
        name = "lib"
        version = "0.1.0"

        [package.build]
        backend = { name = "foobar", version = "*" }

        [package.host-dependencies]
        ghost = { workspace = true }
        "#,
        ));
    }

    #[test]
    fn test_inherited_entry_cannot_restate_version() {
        // Restating `version` on a `{ workspace = true }` entry is rejected.
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "monorepo"
        channels = []
        platforms = ['linux-64']
        preview = ["pixi-build"]

        [workspace.dependencies]
        numpy = "1.*"

        [package]
        name = "lib"
        version = "0.1.0"

        [package.build]
        backend = { name = "foobar", version = "*" }

        [package.host-dependencies]
        numpy = { workspace = true, version = "2.0" }
        "#,
        ));
    }

    #[test]
    fn test_workspace_false_on_dependency_errors() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "monorepo"
        channels = []
        platforms = ['linux-64']
        preview = ["pixi-build"]

        [package]
        name = "lib"
        version = "0.1.0"

        [package.build]
        backend = { name = "foobar", version = "*" }

        [package.host-dependencies]
        numpy = { workspace = false }
        "#,
        ));
    }

    #[test]
    fn test_target_workspace_dependencies() {
        assert_snapshot!(expect_parse_warnings(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ['osx-64']
        preview = ["pixi-build"]

        [package]

        [package.build]
        backend = { name = "foobar", version = "*" }

        [target.osx-64.build-dependencies]
        "#,
        ));
    }

    #[test]
    fn test_mismatching_target_selector() {
        assert_snapshot!(expect_parse_warnings(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ['win-64']

        [target.osx-64.dependencies]
        "#,
        ));
    }

    #[test]
    fn test_mismatching_multi_target_selector() {
        assert_snapshot!(expect_parse_warnings(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ['win-64']

        [target.osx.dependencies]
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

        [host-dependencies]
        foo = "*"
        "#,
            ),
            @r#"
         ⚠ The `host-dependencies` field is deprecated. Use `dependencies` instead.
          ╭─[pixi.toml:7:9]
        6 │
        7 │ ╭─▶         [host-dependencies]
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

        [build-dependencies]
        bar = "*"
        "#,
            ),
            @r#"
         ⚠ The `build-dependencies` field is deprecated. Use `dependencies` instead.
          ╭─[pixi.toml:7:9]
        6 │
        7 │ ╭─▶         [build-dependencies]
        8 │ ├─▶         bar = "*"
          · ╰──── replace this with 'dependencies'
        9 │
          ╰────
        "#
        );
    }

    #[test]
    fn test_system_requirements_deprecation_warning() {
        assert_snapshot!(
            expect_parse_warnings(
                r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ['linux-64']

        [system-requirements]
        cuda = "12"
        "#,
            ),
            @r#"
         ⚠ the `[system-requirements]` table is deprecated in favor of virtual packages on `platforms`
          ╭─[pixi.toml:7:9]
        6 │
        7 │ ╭─▶         [system-requirements]
        8 │ ├─▶         cuda = "12"
          · ╰──── declare these on the `platforms` entries instead
        9 │
          ╰────
         help: e.g. platforms = [{ platform = "linux-64", cuda = "12" }]
        "#
        );
    }

    #[test]
    fn test_feature_system_requirements_deprecation_warning() {
        assert_snapshot!(
            expect_parse_warnings(
                r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ['linux-64']

        [feature.cuda.system-requirements]
        cuda = "12"

        [environments]
        cuda = ["cuda"]
        "#,
            ),
            @r#"
         ⚠ the `[system-requirements]` table is deprecated in favor of virtual packages on `platforms`
          ╭─[pixi.toml:7:9]
        6 │
        7 │ ╭─▶         [feature.cuda.system-requirements]
        8 │ ├─▶         cuda = "12"
          · ╰──── declare these on the `platforms` entries instead
        9 │
          ╰────
         help: e.g. platforms = [{ platform = "linux-64", cuda = "12" }]
        "#
        );
    }

    #[test]
    fn test_unused_features() {
        assert_snapshot!(expect_parse_warnings(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = ['osx-64']

        [feature.foobar.dependencies]

        [feature.generic.target.osx.dependencies]
        "#,
        ));
    }

    #[test]
    fn test_expression_selector_rejected_in_workspace_target() {
        // `if(...)` expression selectors are only valid inside the `[package]`
        // dependency tables; in a workspace `[target.*]` they must be rejected
        // with a hint pointing users at the package dependency tables.
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ['linux-64']

        [target."if(host_platform == build_platform)".dependencies]
        foo = "*"
        "#,
        ), @r###"
          × `if(host_platform == build_platform)` is not a valid target selector. Expression selectors (`if(...)`) are only supported inside the `[package]` dependency tables (e.g. `[package.build-
          │ dependencies."if(host_platform == 'linux-64')"]`); `[target.*]` accepts platform names only
           ╭─[pixi.toml:7:18]
         6 │
         7 │         [target."if(host_platform == build_platform)".dependencies]
           ·                  ───────────────────────────────────
         8 │         foo = "*"
           ╰────
        "###);
    }

    #[test]
    fn test_unknown_feature() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []

        [environments]
        foobar = ["unknown"]
        "#,
        ));
    }

    #[test]
    fn test_unknown_feature2() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []

        [environments]
        foobar = { features = ["unknown"] }
        "#,
        ));
    }

    #[test]
    fn test_duplicate_feature() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []

        [feature.foobar.dependencies]
        [feature.duplicate.dependencies]

        [environments]
        foobar = ["duplicate", "foobar", "duplicate"]
        "#,
        ));
    }

    #[test]
    fn test_conflicting_system_requirements() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []

        [feature.foo.system-requirements]
        archspec = "foo"

        [feature.bar.system-requirements]
        archspec = "bar"

        [environments]
        foobar = ["foo", "bar"]
        "#,
        ));
    }

    #[test]
    fn test_conflicting_pypi_options() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []

        [feature.foo.pypi-options]
        index-url = "https://google.com"

        [feature.bar.pypi-options]
        index-url = "https://prefix.dev"

        [environments]
        foobar = ["foo", "bar"]
        "#,
        ));
    }

    #[test]
    fn test_redefine_default_feature() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []

        [feature.default.dependencies]
        "#,
        ));
    }

    #[test]
    fn test_reserved_env_feature_name() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []

        [feature."env:dev".dependencies]
        git = "*"
        "#,
        ), @r###"
          × feature names starting with 'env:' are reserved for environments that define their content inline
           ╭─[pixi.toml:7:19]
         6 │
         7 │         [feature."env:dev".dependencies]
           ·                   ───────
         8 │         git = "*"
           ╰────
        "###);
    }

    #[test]
    fn test_environment_inline_dependencies() {
        let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = ["linux-64"]

        [environments.dev.dependencies]
        git = "*"
        "#,
            Path::new(""),
        )
        .unwrap();

        // Defining dependencies inline auto-declares the environment and
        // prepends the implicit `env:dev` feature to its feature list.
        let env_feature = FeatureName::environment(&EnvironmentName::Named("dev".to_string()));
        let dev = manifest.environment("dev").expect("dev environment exists");
        assert_eq!(dev.features, vec![env_feature.clone()]);

        // The implicit feature carries the inline dependency.
        let feature = manifest
            .feature(&env_feature)
            .expect("the inline feature exists");
        let deps = feature.dependencies(crate::SpecType::Run, None).unwrap();
        assert!(deps.contains_key("git"));
    }

    #[test]
    fn test_environment_inline_dependencies_with_features() {
        let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = ["linux-64"]

        [feature.python.dependencies]
        python = "*"

        [environments.dev]
        features = ["python"]
        dependencies = { git = "*" }
        "#,
            Path::new(""),
        )
        .unwrap();

        // The implicit feature is prepended before the referenced features.
        let dev = manifest.environment("dev").expect("dev environment exists");
        assert_eq!(
            dev.features,
            vec![
                FeatureName::environment(&EnvironmentName::Named("dev".to_string())),
                FeatureName::from("python"),
            ]
        );
    }

    #[test]
    fn test_environment_without_inline_content_has_no_feature() {
        let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = ["linux-64"]

        [feature.python.dependencies]
        python = "*"

        [environments]
        dev = { features = ["python"] }
        "#,
            Path::new(""),
        )
        .unwrap();

        // A purely compositional environment does not synthesize a feature.
        assert!(
            manifest
                .feature(&FeatureName::environment(&EnvironmentName::Named(
                    "dev".to_string()
                )))
                .is_none()
        );
        let dev = manifest.environment("dev").expect("dev environment exists");
        assert_eq!(dev.features, vec![FeatureName::from("python")]);
    }

    #[test]
    fn test_environment_default_inline_dependencies_rejected() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = ["linux-64"]

        [environments.default.dependencies]
        git = "*"
        "#,
        ), @r###"
          × The 'default' environment cannot define dependencies inline
          help: Add these dependencies to the top-level tables (for example '[dependencies]'); they are part of the default environment.
        "###);
    }

    #[test]
    fn test_environment_inline_full_content() {
        let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
        [workspace]
        name = "foo"
        channels = ["conda-forge"]
        platforms = ["linux-64", "osx-64"]

        [environments.dev]
        channels = ["bioconda"]
        platforms = ["linux-64"]
        dependencies = { git = "*" }
        pypi-dependencies = { requests = "*" }

        [environments.dev.tasks]
        greet = "echo hi"
        "#,
            Path::new(""),
        )
        .unwrap();

        let feature = manifest
            .feature(&FeatureName::environment(&EnvironmentName::Named(
                "dev".to_string(),
            )))
            .expect("the inline feature exists");

        assert!(
            feature
                .dependencies(crate::SpecType::Run, None)
                .unwrap()
                .contains_key("git")
        );
        assert!(!feature.pypi_dependencies(None).unwrap().is_empty());
        assert!(feature.channels.is_some());
        assert!(feature.platforms.is_some());
        assert_eq!(feature.targets.default().tasks.len(), 1);
    }

    #[test]
    fn test_environment_inline_host_dependencies_rejected() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = ["linux-64"]

        [environments.dev.host-dependencies]
        git = "*"
        "#,
        ), @r###"
          × Unexpected keys, expected only 'features', 'solve-group', 'no-default-feature', 'platforms', 'channels', 'channel-priority', 'solve-strategy', 'target', 'dependencies', 'pypi-dependencies',
          │ 'dev', 'constraints', 'activation', 'tasks', 'pypi-options'
           ╭─[pixi.toml:7:27]
         6 │
         7 │         [environments.dev.host-dependencies]
           ·                           ────────┬────────
           ·                                   ╰── 'host-dependencies' was not expected here
         8 │         git = "*"
           ╰────
          help: Did you mean 'dependencies'?
        "###);
    }

    #[test]
    fn test_environment_feature_reference_rejected() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = ["linux-64"]

        [environments.dev.dependencies]
        git = "*"

        [environments.test]
        features = ["env:dev"]
        "#,
        ), @r###"
          × The feature 'env:dev' cannot be referenced: names starting with 'env:' refer to content defined inline on an environment
            ╭─[pixi.toml:11:22]
         10 │         [environments.test]
         11 │         features = ["env:dev"]
            ·                      ───────
         12 │
            ╰────
          help: Content defined inline on an environment is private to that environment. Define a named feature to share content between environments.
        "###);
    }

    #[test]
    fn test_environment_feature_self_reference_rejected() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = ["linux-64"]

        [environments.dev]
        features = ["env:dev"]
        dependencies = { git = "*" }
        "#,
        ), @r###"
          × The feature 'env:dev' cannot be referenced: names starting with 'env:' refer to content defined inline on an environment
           ╭─[pixi.toml:8:22]
         7 │         [environments.dev]
         8 │         features = ["env:dev"]
           ·                      ───────
         9 │         dependencies = { git = "*" }
           ╰────
          help: Content defined inline on an environment is private to that environment. Define a named feature to share content between environments.
        "###);
    }

    #[test]
    fn test_parse_dev_path() {
        let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ["linux-64"]

        [dev]
        test-package = { path = "../test-package" }
        "#,
            Path::new(""),
        )
        .unwrap();

        let dev_deps = manifest
            .default_feature()
            .dev_dependencies(None)
            .expect("should have dev dependencies");

        assert_eq!(dev_deps.iter().count(), 1);
        assert!(dev_deps.contains_key("test-package"));
    }

    #[test]
    fn test_parse_dev_git() {
        let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ["linux-64"]

        [dev]
        my-lib = { git = "https://github.com/example/my-lib.git", branch = "main" }
        "#,
            Path::new(""),
        )
        .unwrap();

        let dev_deps = manifest
            .default_feature()
            .dev_dependencies(None)
            .expect("should have dev dependencies");

        assert_eq!(dev_deps.iter().count(), 1);
        assert!(dev_deps.contains_key("my-lib"));
    }

    #[test]
    fn test_parse_dev_multiple() {
        let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ["linux-64"]

        [dev]
        pkg-a = { path = "../pkg-a" }
        pkg-b = { git = "https://github.com/example/pkg-b.git" }
        pkg-c = { url = "https://example.com/pkg-c.tar.gz" }
        "#,
            Path::new(""),
        )
        .unwrap();

        let dev_deps = manifest
            .default_feature()
            .dev_dependencies(None)
            .expect("should have develop dependencies");

        assert_eq!(dev_deps.iter().count(), 3);
        assert!(dev_deps.contains_key("pkg-a"));
        assert!(dev_deps.contains_key("pkg-b"));
        assert!(dev_deps.contains_key("pkg-c"));
    }

    #[test]
    fn test_parse_feature_dev() {
        let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ["linux-64"]

        [feature.extra.dev]
        feature-pkg = { path = "../feature-pkg" }

        [environments]
        default = []
        extra = ["extra"]
        "#,
            Path::new(""),
        )
        .unwrap();

        // Default feature should not have develop dependencies
        assert!(manifest.default_feature().dev_dependencies(None).is_none());

        // Extra feature should have develop dependencies
        let extra_feature = manifest
            .feature(&FeatureName::from("extra"))
            .expect("extra feature should exist");
        let dev_deps = extra_feature
            .dev_dependencies(None)
            .expect("should have develop dependencies");

        assert_eq!(dev_deps.iter().count(), 1);
        assert!(dev_deps.contains_key("feature-pkg"));
    }

    #[test]
    fn test_parse_target_dev() {
        let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ["linux-64", "win-64"]

        [target.linux-64.dev]
        linux-pkg = { path = "../linux-pkg" }

        [target.win-64.dev]
        windows-pkg = { path = "../windows-pkg" }
        "#,
            Path::new(""),
        )
        .unwrap();

        let linux64 = PixiPlatform::from_subdir(Platform::Linux64);
        let linux_deps = manifest
            .default_feature()
            .dev_dependencies(Some(&linux64))
            .expect("should have linux dev dependencies");

        assert_eq!(linux_deps.iter().count(), 1);
        assert!(linux_deps.contains_key("linux-pkg"));

        let win64 = PixiPlatform::from_subdir(Platform::Win64);
        let windows_deps = manifest
            .default_feature()
            .dev_dependencies(Some(&win64))
            .expect("should have windows dev dependencies");

        assert_eq!(windows_deps.iter().count(), 1);
        assert!(windows_deps.contains_key("windows-pkg"));
    }

    #[test]
    fn test_parse_dev_invalid_no_source_type() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ["linux-64"]

        [dev]
        bad-pkg = { subdirectory = "subdir" }
        "#,
        ));
    }

    #[test]
    fn test_parse_dev_invalid_multiple_sources() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "test"
        channels = []
        platforms = ["linux-64"]

        [dev]
        bad-pkg = { path = "../path", git = "https://github.com/example/repo.git" }
        "#,
        ));
    }

    #[test]
    fn test_project_deprecation_warning() {
        assert_snapshot!(
            expect_parse_warnings(
            r#"
        [project]
        name = "foo"
        channels = []
        "#,
            ),
            @r#"
         ⚠ The `project` field is deprecated. Use `workspace` instead.
          ╭─[pixi.toml:2:9]
        1 │
        2 │ ╭─▶         [project]
        3 │ │           name = "foo"
        4 │ ├─▶         channels = []
          · ╰──── replace this with 'workspace'
        5 │
          ╰────
        "#
        );
    }
}
