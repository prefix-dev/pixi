use std::collections::HashMap;

use indexmap::IndexMap;
use pixi_toml::{Same, TomlHashMap, TomlIndexMap, TomlWith};
use toml_span::{
    de_helpers::{expected, TableHelper},
    value::ValueInner,
    DeserError, Value,
};

use crate::{
    environment::EnvironmentIdx,
    error::FeatureNotEnabled,
    manifests::PackageManifest,
    pypi::{pypi_options::PypiOptions, PyPiPackageName},
    toml::{
        environment::TomlEnvironmentList, task::TomlTask, warning::WithWarnings,
        ExternalPackageProperties, ExternalWorkspaceProperties, TomlFeature, TomlPackage,
        TomlTarget, TomlWorkspace, Warning,
    },
    utils::{package_map::UniquePackageMap, PixiSpanned},
    Activation, Environment, EnvironmentName, Environments, Feature, FeatureName,
    KnownPreviewFeature, PyPiRequirement, SolveGroups, SystemRequirements, TargetSelector, Targets,
    Task, TaskName, TomlError, WorkspaceManifest,
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
    pub pypi_dependencies: Option<PixiSpanned<IndexMap<PyPiPackageName, PyPiRequirement>>>,

    /// Additional information to activate an environment.
    pub activation: Option<PixiSpanned<Activation>>,

    /// Target specific tasks to run in the environment
    pub tasks: Option<PixiSpanned<HashMap<TaskName, Task>>>,

    /// The features defined in the project.
    pub feature: Option<PixiSpanned<IndexMap<FeatureName, TomlFeature>>>,

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

    /// Assume that the manifest is a workspace manifest and convert it as such.
    ///
    /// If the manifest also contains a package section that will be converted
    /// as well.
    pub fn into_package_manifest(
        self,
        external: ExternalPackageProperties,
        workspace: &WorkspaceManifest,
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

        let package = package.into_manifest(external, workspace)?;
        Ok((package, Vec::new()))
    }

    /// Assume that the manifest is a workspace manifest and convert it as such.
    ///
    /// If the manifest also contains a package section that will be converted
    /// as well.
    pub fn into_workspace_manifest(
        self,
        external: ExternalWorkspaceProperties,
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
            let WithWarnings {
                value: workspace_target,
                warnings: mut target_warnings,
            } = target.into_workspace_target(Some(selector.value.clone()), preview)?;
            workspace_targets.insert(selector, workspace_target);
            warnings.append(&mut target_warnings);
        }

        // Construct a default feature
        let default_feature = Feature {
            name: FeatureName::Default,

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
        let features: IndexMap<FeatureName, Feature> =
            IndexMap::from_iter([(FeatureName::Default, default_feature)]);
        let named_features = self
            .feature
            .map(PixiSpanned::into_inner)
            .unwrap_or_default()
            .into_iter()
            .map(|(name, feature)| {
                let WithWarnings {
                    value: feature,
                    warnings: mut feature_warnings,
                } = feature.into_feature(name.clone(), preview)?;
                warnings.append(&mut feature_warnings);
                Ok((name, feature))
            })
            .collect::<Result<IndexMap<FeatureName, Feature>, TomlError>>()?;
        let features = features.into_iter().chain(named_features).collect();

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
        for (name, env) in toml_environments {
            // Decompose the TOML
            let (features, features_source_loc, solve_group, no_default_feature) = match env {
                TomlEnvironmentList::Map(env) => {
                    let (features, features_span) = match env.features {
                        Some(features) => (features.value, features.span),
                        None => (Vec::new(), None),
                    };
                    (
                        features,
                        features_span,
                        env.solve_group,
                        env.no_default_feature,
                    )
                }
                TomlEnvironmentList::Seq(features) => (features, None, None, false),
            };

            let environment_idx = EnvironmentIdx(environments.environments.len());
            environments.by_name.insert(name.clone(), environment_idx);
            environments.environments.push(Some(Environment {
                name,
                features,
                features_source_loc,
                solve_group: solve_group.map(|sg| solve_groups.add(sg, environment_idx)),
                no_default_feature,
            }));
        }

        // Get the name from the [package] section if it's missing from the workspace.
        let project_name = self
            .package
            .as_ref()
            .and_then(|p| p.value.name.as_ref())
            .cloned();

        let workspace = workspace
            .value
            .into_workspace(ExternalWorkspaceProperties {
                name: project_name.or(external.name),
                ..external
            })?;

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

            let workspace = &workspace_manifest.workspace;
            let package = package.into_manifest(
                ExternalPackageProperties {
                    name: Some(workspace.name.clone()),
                    version: workspace.version.clone(),
                    description: workspace.description.clone(),
                    authors: workspace.authors.clone(),
                    license: workspace.license.clone(),
                    license_file: workspace.license_file.clone(),
                    readme: workspace.readme.clone(),
                    homepage: workspace.homepage.clone(),
                    repository: workspace.repository.clone(),
                    documentation: workspace.documentation.clone(),
                },
                &workspace_manifest,
            )?;

            Some(package)
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

#[cfg(test)]
mod test {
    use insta::assert_snapshot;

    use super::*;
    use crate::{toml::FromTomlStr, utils::test_utils::format_parse_error};

    /// A helper function that generates a snapshot of the error message when
    /// parsing a manifest TOML. The error is returned.
    #[must_use]
    pub(crate) fn expect_parse_failure(pixi_toml: &str) -> String {
        let parse_error = <TomlManifest as FromTomlStr>::from_toml_str(pixi_toml)
            .and_then(|manifest| {
                manifest.into_workspace_manifest(ExternalWorkspaceProperties::default())
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
    fn test_missing_version() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []
        preview = ["pixi-build"]

        [package]

        [package.build]
        backend = { name = "foobar", version = "*" }
        "#,
        ));
    }

    #[test]
    fn test_workspace_name_required() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = []
        preview = ["pixi-build"]
        "#,
        ));
    }

    #[test]
    fn test_workspace_name_from_workspace() {
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

        assert_eq!(workspace_manifest.workspace.name, "foo");
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
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = []
        preview = ["pixi-build"]

        [package]

        [package.build]
        backend = { name = "foobar", version = "*" }

        [target.osx-64.build-dependencies]
        "#,
        ));
    }
}
