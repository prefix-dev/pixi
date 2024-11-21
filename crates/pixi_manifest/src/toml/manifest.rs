use std::collections::HashMap;

use indexmap::IndexMap;
use pixi_spec::PixiSpec;
use serde::Deserialize;
use serde_with::serde_as;

use crate::{
    environment::EnvironmentIdx,
    pypi::{pypi_options::PypiOptions, PyPiPackageName},
    toml::{environment::TomlEnvironmentList, TomlWorkspace},
    utils::PixiSpanned,
    Activation, BuildSection, Environment, EnvironmentName, Environments, Feature, FeatureName,
    PyPiRequirement, SolveGroups, SpecType, SystemRequirements, Target, TargetSelector, Targets,
    Task, TaskName, TomlError, Workspace, WorkspaceManifest,
};

/// Raw representation of a pixi manifest. This is the deserialized form of the
/// manifest without any validation logic applied.
#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TomlManifest {
    #[serde(alias = "project")]
    pub workspace: PixiSpanned<TomlWorkspace>,

    #[serde(default)]
    pub system_requirements: SystemRequirements,

    #[serde(default)]
    pub target: IndexMap<PixiSpanned<TargetSelector>, Target>,

    // HACK: If we use `flatten`, unknown keys will point to the wrong location in the
    // file.  When https://github.com/toml-rs/toml/issues/589 is fixed we should use that
    //
    // Instead we currently copy the keys from the Target deserialize implementation which
    // is really ugly.
    //
    // #[serde(flatten)]
    // default_target: Target,
    #[serde(
        default,
        deserialize_with = "crate::utils::package_map::deserialize_package_map"
    )]
    pub dependencies: IndexMap<rattler_conda_types::PackageName, PixiSpec>,

    #[serde(
        default,
        deserialize_with = "crate::utils::package_map::deserialize_opt_package_map"
    )]
    pub host_dependencies: Option<IndexMap<rattler_conda_types::PackageName, PixiSpec>>,

    #[serde(
        default,
        deserialize_with = "crate::utils::package_map::deserialize_opt_package_map"
    )]
    pub build_dependencies: Option<IndexMap<rattler_conda_types::PackageName, PixiSpec>>,

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
    pub feature: IndexMap<FeatureName, Feature>,

    /// The environments the project can create.
    #[serde(default)]
    pub environments: IndexMap<EnvironmentName, TomlEnvironmentList>,

    /// pypi-options
    #[serde(default)]
    pub pypi_options: Option<PypiOptions>,

    /// The build section
    #[serde(default)]
    pub build: Option<BuildSection>,

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

    /// Converts the raw manifest into a workspace manifest.
    ///
    /// The `name` is used to set the workspace name in the manifest if it is
    /// not set there. A missing name in the manifest is not allowed.
    pub fn into_workspace_manifest(
        mut self,
        name: Option<String>,
    ) -> Result<WorkspaceManifest, TomlError> {
        let mut dependencies = HashMap::from_iter([(SpecType::Run, self.dependencies)]);
        if let Some(host_deps) = self.host_dependencies {
            dependencies.insert(SpecType::Host, host_deps);
        }
        if let Some(build_deps) = self.build_dependencies {
            dependencies.insert(SpecType::Build, build_deps);
        }

        let default_target = Target {
            dependencies,
            pypi_dependencies: self.pypi_dependencies,
            activation: self.activation,
            tasks: self.tasks,
        };

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
            targets: Targets::from_default_and_user_defined(default_target, self.target),
        };

        // Construct the features including the default feature
        let features: IndexMap<FeatureName, Feature> =
            IndexMap::from_iter([(FeatureName::Default, default_feature)]);
        let named_features = self
            .feature
            .into_iter()
            .map(|(name, mut feature)| {
                feature.name = name.clone();
                (name, feature)
            })
            .collect::<IndexMap<FeatureName, Feature>>();
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

        let build = self.build;

        // Raise an error if the workspace name is not set.
        let name = self
            .workspace
            .value
            .name
            .take()
            .or(name)
            .ok_or_else(|| TomlError::NoProjectName(self.workspace.span()))?;

        let workspace = self.workspace.value;
        Ok(WorkspaceManifest {
            workspace: Workspace {
                name,
                version: workspace.version,
                description: workspace.description,
                authors: workspace.authors,
                channels: workspace.channels,
                channel_priority: workspace.channel_priority,
                platforms: workspace.platforms,
                license: workspace.license,
                license_file: workspace.license_file,
                readme: workspace.readme,
                homepage: workspace.homepage,
                repository: workspace.repository,
                documentation: workspace.documentation,
                conda_pypi_map: workspace.conda_pypi_map,
                pypi_options: workspace.pypi_options,
                preview: workspace.preview,
            },
            features,
            environments,
            solve_groups,
            build,
        })
    }
}
