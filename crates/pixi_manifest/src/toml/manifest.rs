use std::collections::HashMap;

use indexmap::IndexMap;
use itertools::chain;
use serde::Deserialize;
use serde_with::serde_as;

use crate::{
    environment::EnvironmentIdx,
    error::{FeatureNotEnabled, InvalidNonPackageDependencies},
    manifests::PackageManifest,
    pypi::{pypi_options::PypiOptions, PyPiPackageName},
    toml::{
        environment::TomlEnvironmentList, ExternalPackageProperties, ExternalWorkspaceProperties,
        PackageError, TomlBuildSystem, TomlFeature, TomlPackage, TomlTarget, TomlWorkspace,
        WorkspaceError,
    },
    utils::{package_map::UniquePackageMap, PixiSpanned},
    Activation, Environment, EnvironmentName, Environments, Feature, FeatureName,
    KnownPreviewFeature, PyPiRequirement, SolveGroups, SystemRequirements, TargetSelector, Targets,
    Task, TaskName, TomlError, WorkspaceManifest,
};

/// Raw representation of a pixi manifest. This is the deserialized form of the
/// manifest without any validation logic applied.
#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TomlManifest {
    #[serde(alias = "project")]
    pub workspace: PixiSpanned<TomlWorkspace>,

    pub package: Option<PixiSpanned<TomlPackage>>,

    #[serde(default)]
    pub system_requirements: SystemRequirements,

    #[serde(default)]
    pub target: IndexMap<PixiSpanned<TargetSelector>, TomlTarget>,

    // HACK: If we use `flatten`, unknown keys will point to the wrong location in the
    // file.  When https://github.com/toml-rs/toml/issues/589 is fixed we should use that
    //
    // Instead we currently copy the keys from the Target deserialize implementation which
    // is really ugly.
    //
    // #[serde(flatten)]
    // default_target: Target,
    #[serde(default)]
    pub dependencies: Option<PixiSpanned<UniquePackageMap>>,

    #[serde(default)]
    pub host_dependencies: Option<PixiSpanned<UniquePackageMap>>,

    #[serde(default)]
    pub build_dependencies: Option<PixiSpanned<UniquePackageMap>>,

    #[serde(default)]
    pub run_dependencies: Option<PixiSpanned<UniquePackageMap>>,

    #[serde(default)]
    pub pypi_dependencies: Option<IndexMap<PyPiPackageName, PyPiRequirement>>,

    /// Additional information to activate an environment.
    #[serde(default)]
    pub activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    #[serde(default)]
    pub tasks: HashMap<TaskName, Task>,

    /// The features defined in the project.
    #[serde(default)]
    pub feature: IndexMap<FeatureName, TomlFeature>,

    /// The environments the project can create.
    #[serde(default)]
    pub environments: IndexMap<EnvironmentName, TomlEnvironmentList>,

    /// pypi-options
    #[serde(default)]
    pub pypi_options: Option<PypiOptions>,

    /// The build section
    #[serde(default)]
    pub build_system: Option<PixiSpanned<TomlBuildSystem>>,

    /// The URI for the manifest schema which is unused by pixi
    #[serde(rename = "$schema")]
    pub _schema: Option<String>,

    /// The tool configuration which is unused by pixi
    #[serde(default, skip_serializing, rename = "tool")]
    pub _tool: serde::de::IgnoredAny,
}

impl TomlManifest {
    /// Parses a toml string into a project manifest.
    pub fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        toml_edit::de::from_str(source).map_err(TomlError::from)
    }

    pub fn is_pixi_build_enabled(&self) -> bool {
        self.workspace
            .value
            .preview
            .is_enabled(KnownPreviewFeature::PixiBuild)
    }

    /// Check if some dependency types are used which will not be used.
    fn check_dependency_usage(&self) -> Result<(), TomlError> {
        // If `pixi-build` is not enabled then we can ignore the checks.
        if !self.is_pixi_build_enabled() {
            return Ok(());
        }

        // If the `[package]` section is present then we can ignore the checks.
        if self.package.is_some() {
            return Ok(());
        }

        // Find all the dependency sections which are not allowed without the
        // `[package]` section.
        let top_level_dependencies = vec![
            self.run_dependencies.as_ref().and_then(PixiSpanned::span),
            self.host_dependencies.as_ref().and_then(PixiSpanned::span),
            self.build_dependencies.as_ref().and_then(PixiSpanned::span),
        ];
        let target_dependencies = self.target.values().flat_map(|t| {
            [
                t.run_dependencies.as_ref().and_then(PixiSpanned::span),
                t.host_dependencies.as_ref().and_then(PixiSpanned::span),
                t.build_dependencies.as_ref().and_then(PixiSpanned::span),
            ]
        });
        let feature_dependencies = self.feature.values().flat_map(|f| {
            let top_level_dependencies = [
                f.host_dependencies.as_ref().and_then(PixiSpanned::span),
                f.build_dependencies.as_ref().and_then(PixiSpanned::span),
            ];
            let target_dependencies = f.target.values().flat_map(|t| {
                [
                    t.host_dependencies.as_ref().and_then(PixiSpanned::span),
                    t.build_dependencies.as_ref().and_then(PixiSpanned::span),
                ]
            });
            chain!(top_level_dependencies, target_dependencies)
        });
        let invalid_dependency_sections = chain!(
            top_level_dependencies,
            target_dependencies,
            feature_dependencies
        )
        .flatten()
        .collect::<Vec<_>>();

        if invalid_dependency_sections.is_empty() {
            Ok(())
        } else {
            Err(InvalidNonPackageDependencies {
                invalid_dependency_sections,
            }
            .into())
        }
    }

    /// Converts the raw manifest into a workspace manifest.
    ///
    /// The `name` is used to set the workspace name in the manifest if it is
    /// not set there. A missing name in the manifest is not allowed.
    pub fn into_manifests(
        self,
        external: ExternalWorkspaceProperties,
    ) -> Result<(WorkspaceManifest, Option<PackageManifest>), TomlError> {
        self.check_dependency_usage()?;

        let preview = &self.workspace.value.preview;
        let pixi_build_enabled = self.is_pixi_build_enabled();

        let default_top_level_target = TomlTarget {
            dependencies: self.dependencies,
            host_dependencies: self.host_dependencies,
            build_dependencies: self.build_dependencies,
            run_dependencies: self.run_dependencies,
            pypi_dependencies: self.pypi_dependencies,
            activation: self.activation,
            tasks: self.tasks,
        };

        let (default_workspace_target, default_package_target) =
            default_top_level_target.into_top_level_targets(preview)?;

        let mut workspace_targets = IndexMap::new();
        let mut package_targets = IndexMap::new();
        for (selector, target) in self.target {
            let (workspace_target, package_target) = target.into_top_level_targets(preview)?;
            if let Some(package_target) = package_target {
                package_targets.insert(selector.clone(), package_target);
            }
            workspace_targets.insert(selector, workspace_target);
        }

        // Construct a default feature
        let default_feature = Feature {
            name: FeatureName::Default,

            // The default feature does not overwrite the platforms or channels from the project
            // metadata.
            platforms: None,
            channels: None,

            channel_priority: self.workspace.value.channel_priority,

            system_requirements: self.system_requirements,

            // Use the pypi-options from the manifest for
            // the default feature
            pypi_options: self.pypi_options,

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
            .into_iter()
            .map(|(name, feature)| {
                let feature = feature.into_feature(name.clone(), preview)?;
                Ok((name, feature))
            })
            .collect::<Result<IndexMap<FeatureName, Feature>, TomlError>>()?;
        let features = features.into_iter().chain(named_features).collect();

        // Construct the environments including the default environment
        let mut environments = Environments::default();
        let mut solve_groups = SolveGroups::default();

        // Add the default environment first if it was not redefined.
        if !self.environments.contains_key(&EnvironmentName::Default) {
            environments.environments.push(Some(Environment::default()));
            environments
                .by_name
                .insert(EnvironmentName::Default, EnvironmentIdx(0));
        }

        // Add all named environments
        for (name, env) in self.environments {
            // Decompose the TOML
            let (features, features_source_loc, solve_group, no_default_feature) = match env {
                TomlEnvironmentList::Map(env) => (
                    env.features.value,
                    env.features.span,
                    env.solve_group,
                    env.no_default_feature,
                ),
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

        let PixiSpanned {
            span: workspace_span,
            value: workspace,
        } = self.workspace;
        let workspace = workspace
            .into_workspace(ExternalWorkspaceProperties {
                name: project_name.or(external.name),
                ..external
            })
            .map_err(|e| match e {
                WorkspaceError::MissingName => {
                    TomlError::MissingField("name".into(), workspace_span)
                }
            })?;

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

            let PixiSpanned {
                value: build_system,
                span: _build_system_span,
            } = self
                .build_system
                .ok_or_else(|| TomlError::MissingField("[build-system]".into(), None))?;

            let package = package
                .into_package(ExternalPackageProperties {
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
                })
                .map_err(|e| match e {
                    PackageError::MissingName => {
                        TomlError::MissingField("name".into(), package_span)
                    }
                    PackageError::MissingVersion => {
                        TomlError::MissingField("version".into(), package_span)
                    }
                })?;

            Some(PackageManifest {
                package,
                build_system: build_system.into_build_system()?,
                targets: Targets::from_default_and_user_defined(
                    default_package_target.unwrap_or_default(),
                    package_targets,
                ),
            })
        } else {
            // If we do have a build-system section we have to error out.
            if let Some(PixiSpanned {
                value: _,
                span: build_system_span,
            }) = self.build_system
            {
                return if !pixi_build_enabled {
                    Err(FeatureNotEnabled::new(
                        format!(
                            "[build-system] section is only allowed when the `{}` feature is enabled",
                            KnownPreviewFeature::PixiBuild
                        ),
                        KnownPreviewFeature::PixiBuild,
                    )
                        .with_opt_span(build_system_span)
                        .into())
                } else {
                    Err(TomlError::Generic(
                        "Cannot use [build-system] without [package]".into(),
                        build_system_span,
                    ))
                };
            }

            None
        };

        let workspace_manifest = WorkspaceManifest {
            workspace,
            features,
            environments,
            solve_groups,
        };

        Ok((workspace_manifest, package_manifest))
    }
}

#[cfg(test)]
mod test {
    use insta::assert_snapshot;

    use super::*;
    use crate::utils::test_utils::expect_parse_failure;

    #[test]
    fn test_build_section_without_preview() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []

        [build-system]
        build-backend = { name = "foobar", version = "*" }
        "#,
        ));
    }

    #[test]
    fn test_build_section_without_package() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []
        preview = ["pixi-build"]

        [build-system]
        build-backend = { name = "foobar", version = "*" }
        "#,
        ));
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

        [build-system]
        build-backend = { name = "foobar", version = "*" }
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

        [build-system]
        build-backend = { name = "foobar", version = "*" }
        "#,
        )
        .unwrap();

        assert_eq!(workspace_manifest.workspace.name, "foo");
    }

    #[test]
    fn test_run_dependencies_without_pixi_build() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = []

        [run-dependencies]
        "#,
        ));
    }

    #[test]
    fn test_run_dependencies_in_target_without_pixi_build() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = []

        [target.win.run-dependencies]
        "#,
        ));
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
}
