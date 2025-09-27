use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use miette::LabeledSpan;
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_toml::{Same, TomlHashMap, TomlIndexMap, TomlWith};
use rattler_conda_types::{Platform, Version};
use toml_span::{
    DeserError, Spanned, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};
use url::Url;

use crate::{
    Activation, Environment, EnvironmentName, Environments, Feature, FeatureName,
    KnownPreviewFeature, SolveGroups, SystemRequirements, TargetSelector, Targets, Task, TaskName,
    TomlError, Warning, WithWarnings, WorkspaceManifest,
    environment::EnvironmentIdx,
    error::{FeatureNotEnabled, GenericError},
    manifests::PackageManifest,
    pypi::pypi_options::PypiOptions,
    toml::{
        PackageDefaults, PlatformSpan, TomlFeature, TomlPackage, TomlTarget, TomlWorkspace,
        WorkspacePackageProperties, create_unsupported_selector_warning,
        environment::TomlEnvironmentList, task::TomlTask,
    },
    utils::{PixiSpanned, package_map::UniquePackageMap},
};

/// Raw representation of a pixi manifest. This is the deserialized form of the
/// manifest without any validation logic applied.
#[derive(Debug)]
pub struct TomlManifest {
    pub workspace: Option<PixiSpanned<TomlWorkspace>>,
    pub package: Option<PixiSpanned<TomlPackage>>,

    pub system_requirements: Option<PixiSpanned<SystemRequirements>>,
    pub target: Option<PixiSpanned<IndexMap<PixiSpanned<TargetSelector>, TomlTarget>>>,
    pub dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub host_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub pypi_dependencies: Option<PixiSpanned<IndexMap<PypiPackageName, PixiPypiSpec>>>,

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
        root_directory: Option<&Path>,
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
        root_directory: Option<&Path>,
    ) -> Result<(WorkspaceManifest, Option<PackageManifest>, Vec<Warning>), TomlError> {
        let workspace = self
            .workspace
            .ok_or_else(|| TomlError::MissingField("project/workspace".into(), None))?;

        let preview = &workspace.value.preview;
        let pixi_build_enabled = preview.is_enabled(KnownPreviewFeature::PixiBuild);

        let WithWarnings {
            value: default_workspace_target,
            mut warnings,
        } = TomlTarget {
            dependencies: self.dependencies,
            host_dependencies: self.host_dependencies,
            build_dependencies: self.build_dependencies,
            pypi_dependencies: self.pypi_dependencies.map(PixiSpanned::into_inner),
            activation: self.activation.map(PixiSpanned::into_inner),
            tasks: self.tasks.map(PixiSpanned::into_inner).unwrap_or_default(),
            warnings: self.warnings,
        }
        .into_workspace_target(None, preview)?;

        let mut workspace_targets = IndexMap::new();
        for (selector, target) in self.target.map(|t| t.value).unwrap_or_default() {
            // Verify that the target selector matches at least one of the platforms of the
            // workspace.
            let matching_platforms = Platform::all()
                .filter(|p| selector.value.matches(*p))
                .collect::<Vec<_>>();
            if !matching_platforms
                .iter()
                .any(|p| workspace.value.platforms.value.contains(p))
            {
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
            } = target.into_workspace_target(Some(selector.value.clone()), preview)?;
            workspace_targets.insert(selector, workspace_target);
            warnings.append(&mut target_warnings);
        }

        // Construct a default feature
        let default_feature = Feature {
            name: FeatureName::default(),

            // The default feature does not overwrite the platforms or channels from the project
            // metadata.
            platforms: None,
            channels: None,

            channel_priority: workspace.value.channel_priority,

            system_requirements: self
                .system_requirements
                .map(PixiSpanned::into_inner)
                .unwrap_or_default(),

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
                    value: feature,
                    warnings: mut feature_warnings,
                } = feature.into_feature(name.value.clone(), preview, &workspace.value)?;
                warnings.append(&mut feature_warnings);
                feature_name_to_span
                    .entry(name.value.clone().to_string())
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
            let (included_features, features_span, solve_group, no_default_feature) = match env {
                TomlEnvironmentList::Map(env) => {
                    let (features, features_span) = env.features.map_or_else(
                        || (Vec::new(), None),
                        |Spanned { value, span }| (value, Some(span)),
                    );
                    (
                        features,
                        features_span,
                        env.solve_group,
                        env.no_default_feature,
                    )
                }
                TomlEnvironmentList::Seq(features) => {
                    (features.value, Some(features.span), None, false)
                }
            };

            features_used_by_environments
                .extend(included_features.iter().map(|span| span.value.clone()));

            // Verify that the features of the environment actually exist and that they are
            // not defined twice.
            let mut features_seen_where = HashMap::new();
            let mut used_features = Vec::with_capacity(included_features.len());
            for Spanned {
                value: feature_name,
                span,
            } in &included_features
            {
                let Some(feature) = features.get(feature_name.as_str()) else {
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
                        GenericError::new(format!("The feature '{}' is included more than once.", &feature.name))
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
                        .get(&FeatureName::DEFAULT)
                        .expect("default feature must exist"),
                );
            };

            // Ensure that the system requirements of all the features are compatible
            if let Err(e) = used_features
                .iter()
                .map(|feature| &feature.system_requirements)
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

            // Check if there are no conflicts in pypi options between features
            if let Err(err) = used_features
                .iter()
                .filter_map(|feature| {
                    if feature.pypi_options().is_none() {
                        // Use the project default features
                        workspace.value.pypi_options.as_ref()
                    } else {
                        feature.pypi_options()
                    }
                })
                .try_fold(PypiOptions::default(), |acc, opts| acc.union(opts))
            {
                return Err(TomlError::from(
                    GenericError::new(err.to_string())
                        .with_opt_span(features_span.map(Into::into))
                        .with_span_label("while resolving pypi options of features defined here"),
                ));
            }

            let environment_idx = EnvironmentIdx(environments.environments.len());
            environments.by_name.insert(name.clone(), environment_idx);
            environments.environments.push(Some(Environment {
                name,
                features: included_features.into_iter().map(Spanned::take).collect(),
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
                    "The feature '{}' is defined but not used in any environment. Dependencies of unused features are not resolved or checked, and use wildcard (*) version specifiers by default, disregarding any set `pinning-strategy`",
                    feature_name
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
            value: workspace,
        } = workspace.value.into_workspace(
            ExternalWorkspaceProperties {
                name: project_name.or(external.name),
                ..external
            },
            root_directory,
        )?;
        warnings.append(&mut workspace_warnings);

        let workspace_manifest = WorkspaceManifest {
            workspace,
            features,
            environments,
            solve_groups,
        };

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

impl<'de> toml_span::Deserialize<'de> for TomlManifest {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let mut warnings = Vec::new();

        let workspace = if th.contains("workspace") {
            Some(th.required_s("workspace")?.into())
        } else {
            th.optional("project")
        };
        let package = th.optional("package");

        let target = th
            .optional::<TomlWith<_, PixiSpanned<TomlIndexMap<PixiSpanned<TargetSelector>, Same>>>>(
                "target",
            )
            .map(TomlWith::into_inner);

        let dependencies = th.optional("dependencies");
        let host_dependencies = th.optional("host-dependencies");
        let build_dependencies = th.optional("build-dependencies");
        let pypi_dependencies = th
            .optional::<TomlWith<_, PixiSpanned<TomlIndexMap<_, Same>>>>("pypi-dependencies")
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
            pypi_dependencies,
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

    use super::*;
    use crate::{toml::FromTomlStr, utils::test_utils::expect_parse_warnings};

    /// A helper function that generates a snapshot of the error message when
    /// parsing a manifest TOML. The error is returned.
    #[must_use]
    pub(crate) fn expect_parse_failure(pixi_toml: &str) -> String {
        let parse_error = <TomlManifest as FromTomlStr>::from_toml_str(pixi_toml)
            .and_then(|manifest| {
                manifest.into_workspace_manifest(
                    ExternalWorkspaceProperties::default(),
                    PackageDefaults::default(),
                    None,
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
    fn test_workspace_name_from_package() {
        let workspace_manifest = WorkspaceManifest::from_toml_str(
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
}
