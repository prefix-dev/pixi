use std::{collections::HashMap, fmt::Display, hash::Hash, str::FromStr};

use indexmap::{Equivalent, IndexMap, IndexSet};
use itertools::Itertools;
use miette::{Context, IntoDiagnostic, SourceCode, miette};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use rattler_conda_types::{ParseStrictness::Strict, Platform, Version, VersionSpec};
use toml_edit::Value;

use crate::{
    DependencyOverwriteBehavior, GetFeatureError, Preview, PrioritizedChannel,
    PypiDependencyLocation, SpecType, SystemRequirements, TargetSelector, Task, TaskName,
    TomlError, WorkspaceTarget, consts,
    environment::{Environment, EnvironmentName},
    environments::Environments,
    error::{DependencyError, UnknownFeature},
    feature::{Feature, FeatureName},
    manifests::document::ManifestDocument,
    solve_group::SolveGroups,
    to_options,
    toml::{
        ExternalWorkspaceProperties, FromTomlStr, PackageDefaults, TomlManifest,
        WorkspacePackageProperties,
    },
    utils::WithSourceCode,
    workspace::Workspace,
};

/// Holds the parsed content of the workspace part of a pixi manifest. This
/// describes the part related to the workspace only.
#[derive(Debug, Default, Clone)]
pub struct WorkspaceManifest {
    /// Information about the project
    pub workspace: Workspace,

    /// All the features defined in the project.
    pub features: IndexMap<FeatureName, Feature>,

    /// All the environments defined in the project.
    pub environments: Environments,

    /// The solve groups that are part of the project.
    pub solve_groups: SolveGroups,
}

impl WorkspaceManifest {
    /// Parses a TOML string into a [`WorkspaceManifest`].
    pub fn from_toml_str<S: AsRef<str> + SourceCode>(
        source: S,
    ) -> Result<Self, WithSourceCode<TomlError, S>> {
        TomlManifest::from_toml_str(source.as_ref())
            .and_then(|manifest| {
                manifest.into_workspace_manifest(
                    ExternalWorkspaceProperties::default(),
                    PackageDefaults::default(),
                    None,
                )
            })
            .map(|manifests| manifests.0)
            .map_err(|e| WithSourceCode { source, error: e })
    }

    /// Returns the default feature.
    ///
    /// This is the feature that is added implicitly by the tables at the root
    /// of the project manifest.
    pub fn default_feature(&self) -> &Feature {
        self.features
            .get(&FeatureName::DEFAULT)
            .expect("default feature should always exist")
    }

    /// Returns a mutable reference to the default feature.
    pub(crate) fn default_feature_mut(&mut self) -> &mut Feature {
        self.features
            .get_mut(&FeatureName::DEFAULT)
            .expect("default feature should always exist")
    }

    /// Returns the mutable feature with the given name or `Err` if it does not
    /// exist.
    pub fn feature_mut<Q>(&mut self, name: &Q) -> miette::Result<&mut Feature>
    where
        Q: ?Sized + Hash + Equivalent<FeatureName> + Display,
    {
        self.features.get_mut(name).ok_or_else(|| {
            miette!(
                "Feature {} does not exist",
                consts::FEATURE_STYLE.apply_to(name)
            )
        })
    }

    /// Returns the mutable feature with the given name
    pub fn get_or_insert_feature_mut(&mut self, name: &FeatureName) -> &mut Feature {
        self.features
            .entry(name.clone())
            .or_insert_with(|| Feature::new(name.clone()))
    }

    /// Returns the default environment
    ///
    /// This is the environment that is added implicitly as the environment with
    /// only the default feature. The default environment can be overwritten
    /// by a environment named `default`.
    pub fn default_environment(&self) -> &Environment {
        let envs = &self.environments;
        envs.find(&EnvironmentName::Named(String::from(
            consts::DEFAULT_ENVIRONMENT_NAME,
        )))
        .or_else(|| envs.find(&EnvironmentName::Default))
        .expect("default environment should always exist")
    }

    /// Returns the environment with the given name or `None` if it does not
    /// exist.
    pub fn environment<Q>(&self, name: &Q) -> Option<&Environment>
    where
        Q: ?Sized + Hash + Equivalent<EnvironmentName>,
    {
        self.environments.find(name)
    }

    /// Returns a hashmap of the tasks that should run only the given platform.
    /// If the platform is `None`, only the default targets tasks are
    /// returned.
    pub fn tasks(
        &self,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<HashMap<TaskName, &Task>, GetFeatureError> {
        Ok(self
            .features
            .get(feature_name)
            // Return error if feature does not exist
            .ok_or(GetFeatureError::FeatureDoesNotExist(feature_name.clone()))?
            .targets
            .resolve(platform)
            .rev()
            .flat_map(|target| target.tasks.iter())
            .map(|(name, task)| (name.clone(), task))
            .collect())
    }

    /// Returns a mutable reference to a [`WorkspaceTarget`], creating it if
    /// needed.
    pub fn get_or_insert_target_mut(
        &mut self,
        platform: Option<Platform>,
        name: Option<&FeatureName>,
    ) -> &mut WorkspaceTarget {
        let feature = match name {
            Some(feature) => self.get_or_insert_feature_mut(feature),
            None => self.default_feature_mut(),
        };
        feature
            .targets
            .for_opt_target_or_default_mut(platform.map(TargetSelector::from).as_ref())
    }

    /// Returns a mutable reference to a [`WorkspaceTarget`]. Returns `None` if
    /// the target doesnt exist.
    pub fn target_mut(
        &mut self,
        platform: Option<Platform>,
        name: &FeatureName,
    ) -> Option<&mut WorkspaceTarget> {
        self.feature_mut(name)
            .unwrap()
            .targets
            .for_opt_target_mut(platform.map(TargetSelector::Platform).as_ref())
    }

    /// Returns the feature with the given name or `None` if it does not exist.
    pub fn feature<Q>(&self, name: &Q) -> Option<&Feature>
    where
        Q: ?Sized + Hash + Equivalent<FeatureName>,
    {
        self.features.get(name)
    }

    /// Returns the preview field of the project
    pub fn preview(&self) -> &Preview {
        &self.workspace.preview
    }

    /// Returns true if any of the features has pypi dependencies defined.
    ///
    /// This also returns true if the `pypi-dependencies` key is defined but
    /// empty.
    pub fn has_pypi_dependencies(&self) -> bool {
        self.features
            .values()
            .flat_map(|f| f.targets.targets())
            .any(|f| f.pypi_dependencies.is_some())
    }

    /// Returns default values for the external package properties.
    pub(crate) fn workspace_package_properties(&self) -> WorkspacePackageProperties {
        WorkspacePackageProperties {
            name: self.workspace.name.clone(),
            version: self.workspace.version.clone(),
            description: self.workspace.description.clone(),
            repository: self.workspace.repository.clone(),
            license: self.workspace.license.clone(),
            license_file: self.workspace.license_file.clone(),
            readme: self.workspace.readme.clone(),
            authors: self.workspace.authors.clone(),
            documentation: self.workspace.documentation.clone(),
            homepage: self.workspace.homepage.clone(),
        }
    }
}

/// A mutable context that allows modifying the workspace manifest both in
/// memory and on disk.
pub struct WorkspaceManifestMut<'a> {
    pub workspace: &'a mut WorkspaceManifest,
    pub document: &'a mut ManifestDocument,
}

impl WorkspaceManifestMut<'_> {
    /// Add a task to the project.
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn add_task(
        &mut self,
        name: TaskName,
        task: Task,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Check if the task already exists
        if let Ok(tasks) = self.workspace.tasks(platform, feature_name) {
            if tasks.contains_key(&name) {
                miette::bail!("task {} already exists", name);
            }
        }

        // Add the task to the Toml manifest
        self.document
            .add_task(name.as_str(), task.clone(), platform, feature_name)?;

        // Add the task to the manifest
        self.workspace
            .get_or_insert_target_mut(platform, Some(feature_name))
            .tasks
            .insert(name, task);

        Ok(())
    }

    /// Remove a task from the project.
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn remove_task(
        &mut self,
        name: TaskName,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Check if the task exists
        self.workspace
            .tasks(platform, feature_name)?
            .get(&name)
            .ok_or_else(|| miette::miette!("task {} does not exist", name))?;

        // Remove the task from the Toml manifest
        self.document
            .remove_task(name.as_str(), platform, feature_name)?;

        // Remove the task from the internal manifest
        self.workspace
            .feature_mut(feature_name)?
            .targets
            .for_opt_target_mut(platform.map(TargetSelector::from).as_ref())
            .map(|target| target.tasks.remove(&name));

        Ok(())
    }

    /// Adds an environment to the workspace. Overwrites the entry if it already
    /// exists.
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn add_environment(
        &mut self,
        name: String,
        features: Option<Vec<String>>,
        solve_group: Option<String>,
        no_default_feature: bool,
    ) -> miette::Result<()> {
        // Make sure the features exist
        for feature in features.iter().flatten() {
            if self.workspace.features.get(feature.as_str()).is_none() {
                return Err(UnknownFeature::new(feature.to_string(), &*self.workspace).into());
            }
        }

        self.document.add_environment(
            name.clone(),
            features.clone(),
            solve_group.clone(),
            no_default_feature,
        )?;

        let environment_idx = self.workspace.environments.add(Environment {
            name: EnvironmentName::Named(name),
            features: features.unwrap_or_default(),
            solve_group: None,
            no_default_feature,
        });

        if let Some(solve_group) = solve_group {
            self.workspace
                .solve_groups
                .add(solve_group, environment_idx);
        }

        Ok(())
    }

    /// Removes an environment from the project.
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn remove_environment(&mut self, name: &str) -> miette::Result<bool> {
        // Remove the environment from the TOML document
        if !self.document.remove_environment(name)? {
            return Ok(false);
        }

        // Remove the environment from the internal manifest
        let environment_idx = self
            .workspace
            .environments
            .by_name
            .shift_remove(name)
            .expect("environment should exist");

        // Remove the environment from the solve groups
        self.workspace
            .solve_groups
            .iter_mut()
            .for_each(|group| group.environments.retain(|&idx| idx != environment_idx));

        Ok(true)
    }

    /// Add a platform to the project
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn add_platforms<'a>(
        &mut self,
        platforms: impl Iterator<Item = &'a Platform> + Clone,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Get current and new platforms for the feature
        let current = if feature_name.is_default() {
            &mut self.workspace.workspace.platforms
        } else {
            self.workspace
                .get_or_insert_feature_mut(feature_name)
                .platforms_mut()
        };
        let to_add: IndexSet<_> = platforms.cloned().collect();
        let new: IndexSet<_> = to_add.difference(current).cloned().collect();

        // Add the platforms to the manifest
        current.extend(new.clone());

        // Then to the TOML document
        let platforms = self.document.get_array_mut("platforms", feature_name)?;
        for platform in new.iter() {
            platforms.push(platform.to_string());
        }

        Ok(())
    }

    /// Remove the platform(s) from the project
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn remove_platforms(
        &mut self,
        platforms: impl IntoIterator<Item = Platform> + Clone,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Get current platforms and platform to remove for the feature
        let current = if feature_name.is_default() {
            &mut self.workspace.workspace.platforms
        } else {
            self.workspace.feature_mut(feature_name)?.platforms_mut()
        };

        // Check if some platforms are not part of current
        let missing = platforms
            .clone()
            .into_iter()
            .filter(|p| !current.contains(p))
            .collect_vec();
        if !missing.is_empty() {
            return Err(miette::miette!(
                "The following platform{} are not part of {}: {}",
                if missing.len() > 1 { "s are" } else { " is" },
                feature_name,
                missing.into_iter().join(", ")
            ));
        }

        // Remove platforms from the manifest
        current.retain(|p| !platforms.clone().into_iter().contains(p));

        // And from the TOML document
        let retained = current.iter().map(|p| p.to_string()).collect_vec();
        let platforms = self.document.get_array_mut("platforms", feature_name)?;
        platforms.retain(|p| {
            p.as_str()
                .map(|p| retained.contains(&p.to_string()))
                .unwrap_or(false)
        });

        Ok(())
    }

    /// Add a pixi spec to the manifest
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn add_dependency(
        &mut self,
        name: &rattler_conda_types::PackageName,
        spec: &PixiSpec,
        spec_type: SpecType,
        platforms: &[Platform],
        feature_name: &FeatureName,
        overwrite_behavior: DependencyOverwriteBehavior,
    ) -> miette::Result<bool> {
        let mut any_added = false;
        for platform in to_options(platforms) {
            // Add the dependency to the manifest
            match self
                .workspace
                .get_or_insert_target_mut(platform, Some(feature_name))
                .try_add_dependency(name, spec, spec_type, overwrite_behavior)
            {
                Ok(true) => {
                    self.document
                        .add_dependency(name, spec, spec_type, platform, feature_name)?;
                    any_added = true;
                }
                Ok(false) => {}
                Err(e) => return Err(e.into()),
            };
        }
        Ok(any_added)
    }

    /// Removes a dependency based on `SpecType`.
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn remove_dependency(
        &mut self,
        dep: &rattler_conda_types::PackageName,
        spec_type: SpecType,
        platforms: &[Platform],
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        for platform in crate::to_options(platforms) {
            // Remove the dependency from the manifest
            match self
                .workspace
                .target_mut(platform, feature_name)
                .ok_or_else(|| {
                    handle_missing_target(platform.as_ref(), feature_name, consts::DEPENDENCIES)
                })?
                .remove_dependency(dep, spec_type)
            {
                Ok(_) => (),
                Err(DependencyError::NoDependency(e)) => {
                    tracing::warn!("Dependency `{}` doesn't exist", e);
                }
                Err(e) => return Err(e.into()),
            };
            // Remove the dependency from the TOML document
            self.document
                .remove_dependency(dep, spec_type, platform, feature_name)?;
        }
        Ok(())
    }

    /// Add a pypi requirement to the manifest.
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn add_pep508_dependency(
        &mut self,
        (requirement, pixi_req): (&pep508_rs::Requirement, Option<&PixiPypiSpec>),
        platforms: &[Platform],
        feature_name: &FeatureName,
        editable: Option<bool>,
        overwrite_behavior: DependencyOverwriteBehavior,
        location: Option<PypiDependencyLocation>,
    ) -> miette::Result<bool> {
        let mut any_added = false;
        for platform in to_options(platforms) {
            // Add the pypi dependency to the manifest
            match self
                .workspace
                .get_or_insert_target_mut(platform, Some(feature_name))
                .try_add_pep508_dependency(requirement, editable, overwrite_behavior)
            {
                Ok(true) => {
                    self.document.add_pypi_dependency(
                        requirement,
                        pixi_req,
                        platform,
                        feature_name,
                        editable,
                        location,
                    )?;
                    any_added = true;
                }
                Ok(false) => {}
                Err(e) => return Err(e.into()),
            };
        }
        Ok(any_added)
    }

    /// Removes a pypi dependency.
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn remove_pypi_dependency(
        &mut self,
        dep: &PypiPackageName,
        platforms: &[Platform],
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        for platform in crate::to_options(platforms) {
            // Remove the dependency from the manifest
            match self
                .workspace
                .target_mut(platform, feature_name)
                .ok_or_else(|| {
                    handle_missing_target(
                        platform.as_ref(),
                        feature_name,
                        consts::PYPI_DEPENDENCIES,
                    )
                })?
                .remove_pypi_dependency(dep)
            {
                Ok(_) => (),
                Err(DependencyError::NoDependency(e)) => {
                    tracing::warn!("Dependency `{}` doesn't exist", e);
                }
                Err(e) => return Err(e.into()),
            };
            // Remove the dependency from the TOML document
            self.document
                .remove_pypi_dependency(dep, platform, feature_name)?;
        }
        Ok(())
    }

    /// Adds the specified channels to the manifest.
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn add_channels(
        &mut self,
        channels: impl IntoIterator<Item = PrioritizedChannel>,
        feature_name: &FeatureName,
        prepend: bool,
    ) -> miette::Result<()> {
        // First collect all the new channels
        let to_add: IndexSet<_> = channels.into_iter().collect();

        // Get the current channels and update them
        let current = if feature_name.is_default() {
            &mut self.workspace.workspace.channels
        } else {
            self.workspace
                .get_or_insert_feature_mut(feature_name)
                .channels_mut()
        };

        let new: IndexSet<_> = to_add.difference(current).cloned().collect();
        let new_channels: IndexSet<_> = new
            .clone()
            .into_iter()
            .map(|channel| channel.channel)
            .collect();

        // clear channels with modified priority
        current.retain(|c| !new_channels.contains(&c.channel));

        // Create the final channel list in the desired order
        let final_channels = if prepend {
            let mut new_set = new.clone();
            new_set.extend(current.iter().cloned());
            new_set
        } else {
            let mut new_set = current.clone();
            new_set.extend(new.clone());
            new_set
        };

        // Update both the parsed channels and the TOML document
        *current = final_channels.clone();

        // Update the TOML document
        let channels = self.document.get_array_mut("channels", feature_name)?;
        channels.clear();
        for channel in final_channels {
            channels.push(Value::from(channel));
        }

        Ok(())
    }

    /// Remove the specified channels to the manifest.
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn remove_channels(
        &mut self,
        channels: impl IntoIterator<Item = PrioritizedChannel>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Get current channels and channels to remove for the feature
        let current = if feature_name.is_default() {
            &mut self.workspace.workspace.channels
        } else {
            self.workspace.feature_mut(feature_name)?.channels_mut()
        };
        // Get the channels to remove, while checking if they exist
        let to_remove: IndexSet<_> = channels
            .into_iter()
            .map(|c| {
                current
                    .iter()
                    .position(|x| x.channel.to_string() == c.channel.to_string())
                    .ok_or_else(|| miette::miette!("channel {} does not exist", c.channel.as_str()))
                    .map(|_| c.channel.to_string())
            })
            .collect::<Result<_, _>>()?;

        let retained: IndexSet<_> = current
            .iter()
            .filter(|channel| !to_remove.contains(&channel.channel.to_string()))
            .cloned()
            .collect();

        // Remove channels from the manifest
        current.retain(|c| retained.contains(c));
        let current_clone = current.clone();

        // And from the TOML document
        let channels = self.document.get_array_mut("channels", feature_name)?;
        // clear and recreate from current list
        channels.clear();
        for channel in current_clone.iter() {
            channels.push(Value::from(channel.clone()));
        }

        Ok(())
    }

    /// Set the workspace name.
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn set_name(&mut self, name: &str) -> miette::Result<()> {
        self.workspace.workspace.name = Some(name.to_string());
        self.document.set_name(name);
        Ok(())
    }

    /// Set the project description
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn set_description(&mut self, description: &str) -> miette::Result<()> {
        // Update in both the manifest and the toml
        self.workspace.workspace.description = Some(description.to_string());
        self.document.set_description(description);

        Ok(())
    }

    /// Set the project version
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn set_version(&mut self, version: &str) -> miette::Result<()> {
        // Update in both the manifest and the toml
        self.workspace.workspace.version = Some(
            Version::from_str(version)
                .into_diagnostic()
                .context("could not convert version to a valid project version")?,
        );
        self.document.set_version(version);
        Ok(())
    }

    /// Add a system requirement to the project
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn add_system_requirement(
        &mut self,
        system_requirements: SystemRequirements,
        feature_name: &FeatureName,
    ) -> miette::Result<SystemRequirements> {
        // Get the current system requirements
        let current = if feature_name.is_default() {
            &mut self.workspace.default_feature_mut().system_requirements
        } else {
            &mut self
                .workspace
                .get_or_insert_feature_mut(feature_name)
                .system_requirements
        };

        // Replace the system requirements with the new ones
        // All given requirements are replaced, all optional requirements are kept
        let result = current.merge(&system_requirements);

        *current = result.clone();

        // Update the TOML document
        self.document
            .add_system_requirements(&result, feature_name)
            .into_diagnostic()?;

        Ok(result)
    }

    /// Set/Unset the pixi version requirements
    ///
    /// This function modifies both the workspace and the TOML document. Use
    /// `ManifestProvenance::save` to persist the changes to disk.
    pub fn set_requires_pixi(&mut self, version: Option<&str>) -> miette::Result<()> {
        // Update in both the manifest and the toml
        self.workspace.workspace.requires_pixi = match version {
            Some(version) => Some(
                VersionSpec::from_str(version, Strict)
                    .into_diagnostic()
                    .context("could not convert to a valid version spec")?,
            ),
            None => None,
        };
        self.document.set_requires_pixi(version).into_diagnostic()
    }
}

// Handles the target missing error cases
fn handle_missing_target(
    platform: Option<&Platform>,
    feature_name: &FeatureName,
    section: &str,
) -> miette::Report {
    let platform = platform.copied().unwrap_or_else(Platform::current);

    let help = if feature_name.is_default() {
        format!(
            r#"Expected target for `{name}`, e.g.: `[target.{platform}.{section}]`"#,
            name = feature_name,
            platform = platform,
            section = section
        )
    } else {
        format!(
            r#"Expected target for `{name}`, e.g.: `[feature.{name}.target.{platform}.{section}]`"#,
            name = feature_name,
            platform = platform,
            section = section
        )
    };
    miette!(
        help = &help,
        "No target for feature `{name}` found on platform `{platform}`",
        name = feature_name,
        platform = platform
    )
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use indexmap::{IndexMap, IndexSet};
    use insta::{assert_debug_snapshot, assert_snapshot, assert_yaml_snapshot};
    use itertools::Itertools;
    use miette::NarratableReportHandler;
    use pixi_spec::PixiSpec;
    use pixi_test_utils::format_parse_error;
    use rattler_conda_types::{
        MatchSpec, NamedChannelOrUrl, PackageName, ParseStrictness,
        ParseStrictness::{Lenient, Strict},
        Platform, Version, VersionSpec,
    };
    use rstest::rstest;
    use toml_edit::DocumentMut;

    use super::*;
    use crate::{
        ChannelPriority, DependencyOverwriteBehavior, EnvironmentName, FeatureName,
        PrioritizedChannel, SpecType, TargetSelector, Task, TomlError, WorkspaceManifest,
        manifests::document::ManifestDocument,
        pyproject::PyProjectManifest,
        to_options,
        toml::{FromTomlStr, TomlDocument},
        utils::{WithSourceCode, test_utils::expect_parse_failure},
    };

    const PROJECT_BOILERPLATE: &str = r#"
[project]
name = "foo"
version = "0.1.0"
channels = []
platforms = ['win-64', 'osx-64', 'linux-64']
"#;

    const PYPROJECT_BOILERPLATE: &str = r#"
[project]
name = "flask-hello-world-pyproject"
version = "0.1.0"
description = "Example how to get started with flask in a pixi environment."
license = "MIT OR Apache-2.0"
readme = "README.md"
requires-python = ">=3.11"
dependencies = ["flask==2.*"]

[tool.pixi.project]
channels = ["https://prefix.dev/conda-forge"]
platforms = ["linux-64"]

[tool.pixi.tasks]
start = "python -m flask run --port=5050"
"#;

    pub struct Workspace {
        manifest: WorkspaceManifest,
        document: ManifestDocument,
    }

    impl Workspace {
        pub fn editable(&mut self) -> WorkspaceManifestMut<'_> {
            WorkspaceManifestMut {
                workspace: &mut self.manifest,
                document: &mut self.document,
            }
        }
    }

    fn parse_pixi_toml(source: &str) -> Workspace {
        let editable_document = DocumentMut::from_str(source)
            .map(TomlDocument::new)
            .unwrap_or_else(|error| {
                panic!("{}", format_parse_error(source, TomlError::from(error)))
            });

        let manifest = WorkspaceManifest::from_toml_str(source).unwrap_or_else(
            |WithSourceCode { error, source }| panic!("{}", format_parse_error(source, error)),
        );

        Workspace {
            manifest,
            document: ManifestDocument::PixiToml(editable_document),
        }
    }

    fn parse_pyproject_toml(source: &str) -> Workspace {
        let editable_document = DocumentMut::from_str(source)
            .map(TomlDocument::new)
            .unwrap_or_else(|error| {
                panic!("{}", format_parse_error(source, TomlError::from(error)))
            });

        let manifest = PyProjectManifest::from_toml_str(source)
            .unwrap_or_else(|error| panic!("{}", format_parse_error(source, error)))
            .into_workspace_manifest(None)
            .unwrap_or_else(|error| panic!("{}", format_parse_error(source, error)))
            .0;

        Workspace {
            manifest,
            document: ManifestDocument::PyProjectToml(editable_document),
        }
    }

    fn default_channel_config() -> rattler_conda_types::ChannelConfig {
        rattler_conda_types::ChannelConfig::default_with_root_dir(
            std::env::current_dir().expect("Could not retrieve the current directory"),
        )
    }

    #[test]
    fn test_add_pep508_dependency() {
        let mut manifest = parse_pyproject_toml(PYPROJECT_BOILERPLATE);
        let mut manifest = manifest.editable();

        // Add numpy to pyproject
        let requirement = pep508_rs::Requirement::from_str("numpy>=3.12").unwrap();
        manifest
            .add_pep508_dependency(
                (&requirement, None),
                &[],
                &FeatureName::DEFAULT,
                None,
                DependencyOverwriteBehavior::Overwrite,
                None,
            )
            .unwrap();

        assert!(
            manifest
                .workspace
                .default_feature_mut()
                .targets
                .for_opt_target(None)
                .unwrap()
                .pypi_dependencies
                .as_ref()
                .unwrap()
                .get(&PypiPackageName::from_normalized(requirement.name.clone()))
                .is_some()
        );

        // Add numpy to feature in pyproject
        let requirement = pep508_rs::Requirement::from_str("pytest>=3.12").unwrap();
        manifest
            .add_pep508_dependency(
                (&requirement, None),
                &[],
                &FeatureName::from("test"),
                None,
                DependencyOverwriteBehavior::Overwrite,
                None,
            )
            .unwrap();
        assert!(
            manifest
                .workspace
                .feature(&FeatureName::from("test"))
                .unwrap()
                .targets
                .for_opt_target(None)
                .unwrap()
                .pypi_dependencies
                .as_ref()
                .unwrap()
                .get(&PypiPackageName::from_normalized(requirement.name.clone()))
                .is_some()
        );

        assert_snapshot!(manifest.document.to_string());
    }

    #[test]
    fn test_remove_pypi_dependency() {
        let mut manifest = parse_pyproject_toml(PYPROJECT_BOILERPLATE);
        let mut manifest = manifest.editable();

        // Remove flask from pyproject
        let name = PypiPackageName::from_str("flask").unwrap();
        manifest
            .remove_pypi_dependency(&name, &[], &FeatureName::DEFAULT)
            .unwrap();

        assert!(
            manifest
                .workspace
                .default_feature_mut()
                .targets
                .for_opt_target(None)
                .unwrap()
                .pypi_dependencies
                .as_ref()
                .unwrap()
                .get(&name)
                .is_none()
        );

        assert_snapshot!(manifest.document.to_string());
    }

    #[test]
    fn test_target_specific() {
        let contents = format!(
            r#"
        {PROJECT_BOILERPLATE}

        [target.win-64.dependencies]
        foo = "==3.4.5"

        [target.osx-64.dependencies]
        foo = "==1.2.3"
        "#
        );

        let manifest = parse_pixi_toml(&contents).manifest;

        let targets = &manifest.default_feature().targets;
        assert_eq!(
            targets.user_defined_selectors().cloned().collect_vec(),
            vec![
                TargetSelector::Platform(Platform::Win64),
                TargetSelector::Platform(Platform::Osx64),
            ]
        );

        let win64_target = targets
            .for_target(&TargetSelector::Platform(Platform::Win64))
            .unwrap();
        let osx64_target = targets
            .for_target(&TargetSelector::Platform(Platform::Osx64))
            .unwrap();
        assert_eq!(
            win64_target
                .run_dependencies()
                .unwrap()
                .get("foo")
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            "==3.4.5"
        );
        assert_eq!(
            osx64_target
                .run_dependencies()
                .unwrap()
                .get("foo")
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            "==1.2.3"
        );
    }

    #[test]
    fn test_mapped_dependencies() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [dependencies]
            test_map = {{ version = ">=1.2.3", channel="conda-forge", build="py34_0" }}
            test_build = {{ build = "bla" }}
            test_channel = {{ channel = "conda-forge" }}
            test_version = {{ version = ">=1.2.3" }}
            test_version_channel = {{ version = ">=1.2.3", channel = "conda-forge" }}
            test_version_build = {{ version = ">=1.2.3", build = "py34_0" }}
            "#
        );

        let manifest = parse_pixi_toml(&contents).manifest;
        let deps = manifest
            .default_feature()
            .targets
            .default()
            .run_dependencies()
            .unwrap();
        let test_map_spec = deps.get("test_map").unwrap().as_detailed().unwrap();

        assert_eq!(
            test_map_spec.version.as_ref().unwrap().to_string(),
            ">=1.2.3"
        );
        assert_eq!(test_map_spec.build.as_ref().unwrap().to_string(), "py34_0");
        assert_eq!(
            test_map_spec.channel,
            Some(NamedChannelOrUrl::Name("conda-forge".to_string()))
        );

        assert_eq!(
            deps.get("test_build")
                .unwrap()
                .as_detailed()
                .unwrap()
                .build
                .as_ref()
                .unwrap()
                .to_string(),
            "bla"
        );

        let test_channel = deps.get("test_channel").unwrap().as_detailed().unwrap();
        assert_eq!(
            test_channel.channel,
            Some(NamedChannelOrUrl::Name("conda-forge".to_string()))
        );

        let test_version = deps.get("test_version").unwrap().as_detailed().unwrap();
        assert_eq!(
            test_version.version.as_ref().unwrap().to_string(),
            ">=1.2.3"
        );

        let test_version_channel = deps
            .get("test_version_channel")
            .unwrap()
            .as_detailed()
            .unwrap();
        assert_eq!(
            test_version_channel.version.as_ref().unwrap().to_string(),
            ">=1.2.3"
        );
        assert_eq!(
            test_version_channel.channel,
            Some(NamedChannelOrUrl::Name("conda-forge".to_string()))
        );

        let test_version_build = deps
            .get("test_version_build")
            .unwrap()
            .as_detailed()
            .unwrap();
        assert_eq!(
            test_version_build.version.as_ref().unwrap().to_string(),
            ">=1.2.3"
        );
        assert_eq!(
            test_version_build.build.as_ref().unwrap().to_string(),
            "py34_0"
        );
    }

    #[test]
    fn test_dependency_types() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [dependencies]
            my-game = "1.0.0"

            [build-dependencies]
            cmake = "*"

            [host-dependencies]
            sdl2 = "*"
            "#
        );

        let manifest = parse_pixi_toml(&contents).manifest;
        let default_target = manifest.default_feature().targets.default();
        let run_dependencies = default_target.run_dependencies().unwrap();
        let build_dependencies = default_target.build_dependencies().unwrap();
        let host_dependencies = default_target.host_dependencies().unwrap();

        assert_eq!(
            run_dependencies
                .get("my-game")
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            "==1.0.0"
        );
        assert_eq!(
            build_dependencies
                .get("cmake")
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            "*"
        );
        assert_eq!(
            host_dependencies
                .get("sdl2")
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            "*"
        );
    }

    #[test]
    fn test_invalid_target_specific() {
        let examples = [r#"[target.foobar.dependencies]
            invalid_platform = "henk""#];

        assert_snapshot!(expect_parse_failure(&format!(
            "{PROJECT_BOILERPLATE}\n{}",
            examples[0]
        )));
    }

    #[test]
    fn test_invalid_key() {
        insta::with_settings!({snapshot_suffix => "foobar"}, {
            assert_snapshot!(expect_parse_failure(&format!("{PROJECT_BOILERPLATE}\n[foobar]")))
        });

        insta::with_settings!({snapshot_suffix => "hostdependencies"}, {
            assert_snapshot!(expect_parse_failure(&format!("{PROJECT_BOILERPLATE}\n[target.win-64.hostdependencies]")))
        });

        insta::with_settings!({snapshot_suffix => "environment"}, {
            assert_snapshot!(expect_parse_failure(&format!("{PROJECT_BOILERPLATE}\n[environments.INVALID]")))
        });
    }

    #[test]
    fn test_target_specific_tasks() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [tasks]
            test = "test multi"

            [target.win-64.tasks]
            test = "test win"

            [target.linux-64.tasks]
            test = "test linux"
            "#
        );

        let manifest = parse_pixi_toml(&contents).manifest;

        assert_snapshot!(
            manifest
                .default_feature()
                .targets
                .iter()
                .flat_map(|(target, selector)| {
                    let selector_name =
                        selector.map_or_else(|| String::from("default"), ToString::to_string);
                    target.tasks.iter().map(move |(name, task)| {
                        format!(
                            "{}/{} = {:?}",
                            &selector_name,
                            name.as_str(),
                            task.as_single_command(None)
                                .ok()
                                .flatten()
                                .map(|c| c.to_string())
                        )
                    })
                })
                .join("\n")
        );
    }

    #[test]
    fn test_invalid_task_list() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [tasks]
            test = ["invalid", "task"]
            "#
        );

        let WithSourceCode { error, source } =
            WorkspaceManifest::from_toml_str(contents).unwrap_err();
        assert_snapshot!(format_parse_error(&source, error));
    }

    #[test]
    fn test_python_dependencies() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [pypi-dependencies]
            foo = ">=3.12"
            bar = {{ version=">=3.12", extras=["baz"] }}
            "#
        );

        let manifest = parse_pixi_toml(&contents).manifest;
        assert_snapshot!(
            manifest
                .default_feature()
                .targets
                .default()
                .pypi_dependencies
                .clone()
                .into_iter()
                .flat_map(|d| d.into_iter())
                .map(|(name, spec)| format!(
                    "{} = {}",
                    name.as_source(),
                    toml_edit::Value::from(spec)
                ))
                .join("\n")
        );
    }

    #[test]
    fn test_pypi_options_default_feature() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [project.pypi-options]
            index-url = "https://pypi.org/simple"
            extra-index-urls = ["https://pypi.org/simple2"]
            [[project.pypi-options.find-links]]
            path = "../foo"
            [[project.pypi-options.find-links]]
            url = "https://example.com/bar"
            "#
        );

        let manifest = parse_pixi_toml(&contents).manifest;
        assert_yaml_snapshot!(manifest.workspace.pypi_options.clone().unwrap());
    }

    #[test]
    fn test_pypy_options_project_and_default_feature() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [project.pypi-options]
            extra-index-urls = ["https://pypi.org/simple2"]

            [pypi-options]
            extra-index-urls = ["https://pypi.org/simple3"]
            "#
        );

        let manifest = parse_pixi_toml(&contents).manifest;
        assert_yaml_snapshot!(manifest.workspace.pypi_options.clone().unwrap());
    }

    #[test]
    fn test_tool_deserialization() {
        let contents = r#"
        [project]
        name = "foo"
        channels = []
        platforms = []
        [tool.ruff]
        test = "test"
        test1 = ["test"]
        test2 = { test = "test" }

        [tool.ruff.test3]
        test = "test"

        [tool.poetry]
        test = "test"
        "#;
        let _ = parse_pixi_toml(contents);
    }

    #[test]
    fn test_build_variants() {
        let contents = r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []

        [workspace.build-variants]
        python = ["3.10.*", "3.11.*"]

        [workspace.target.win-64.build-variants]
        python = ["1.0.*"]
        "#;
        let manifest = parse_pixi_toml(contents).manifest;
        println!("{:?}", manifest.workspace.build_variants);
        let resolved_linux = manifest
            .workspace
            .build_variants
            .resolve(Some(Platform::Linux64))
            .collect::<Vec<_>>();
        assert_debug_snapshot!(resolved_linux);

        let resolved_win = manifest
            .workspace
            .build_variants
            .resolve(Some(Platform::Win64))
            .collect::<Vec<_>>();
        assert_debug_snapshot!(resolved_win);
    }

    #[test]
    fn test_activation_env() {
        let contents = r#"
            [project]
            name = "foo"
            channels = []
            platforms = ["win-64", "linux-64"]

            [activation.env]
            FOO = "main"

            [target.win-64.activation]
            env = { FOO = "win-64" }

            [target.linux-64.activation.env]
            FOO = "linux-64"

            [feature.bar.activation]
            env = { FOO = "bar" }

            [feature.bar.target.win-64.activation]
            env = { FOO = "bar-win-64" }

            [feature.bar.target.linux-64.activation]
            env = { FOO = "bar-linux-64" }
            "#;

        let manifest = parse_pixi_toml(contents).manifest;
        let default_targets = &manifest.default_feature().targets;
        let default_activation_env = default_targets
            .default()
            .activation
            .as_ref()
            .and_then(|a| a.env.as_ref());
        let win64_activation_env = default_targets
            .for_target(&TargetSelector::Platform(Platform::Win64))
            .unwrap()
            .activation
            .as_ref()
            .and_then(|a| a.env.as_ref());
        let linux64_activation_env = default_targets
            .for_target(&TargetSelector::Platform(Platform::Linux64))
            .unwrap()
            .activation
            .as_ref()
            .and_then(|a| a.env.as_ref());

        assert_eq!(
            default_activation_env,
            Some(&IndexMap::from([(
                String::from("FOO"),
                String::from("main")
            )]))
        );
        assert_eq!(
            win64_activation_env,
            Some(&IndexMap::from([(
                String::from("FOO"),
                String::from("win-64")
            )]))
        );
        assert_eq!(
            linux64_activation_env,
            Some(&IndexMap::from([(
                String::from("FOO"),
                String::from("linux-64")
            )]))
        );

        // Check that the feature activation env is set correctly
        let feature_targets = &manifest.feature(&FeatureName::from("bar")).unwrap().targets;
        let feature_activation_env = feature_targets
            .default()
            .activation
            .as_ref()
            .and_then(|a| a.env.as_ref());
        let feature_win64_activation_env = feature_targets
            .for_target(&TargetSelector::Platform(Platform::Win64))
            .unwrap()
            .activation
            .as_ref()
            .and_then(|a| a.env.as_ref());
        let feature_linux64_activation_env = feature_targets
            .for_target(&TargetSelector::Platform(Platform::Linux64))
            .unwrap()
            .activation
            .as_ref()
            .and_then(|a| a.env.as_ref());

        assert_eq!(
            feature_activation_env,
            Some(&IndexMap::from([(
                String::from("FOO"),
                String::from("bar")
            )]))
        );
        assert_eq!(
            feature_win64_activation_env,
            Some(&IndexMap::from([(
                String::from("FOO"),
                String::from("bar-win-64")
            )]))
        );
        assert_eq!(
            feature_linux64_activation_env,
            Some(&IndexMap::from([(
                String::from("FOO"),
                String::from("bar-linux-64")
            )]))
        );
    }

    fn test_remove(
        file_contents: &str,
        name: &str,
        kind: SpecType,
        platforms: &[Platform],
        feature_name: &FeatureName,
    ) {
        let mut manifest = parse_pixi_toml(file_contents);
        let mut manifest = manifest.editable();

        // Initially the dependency should exist
        for platform in to_options(platforms) {
            assert!(
                manifest
                    .workspace
                    .feature_mut(feature_name)
                    .unwrap()
                    .targets
                    .for_opt_target(platform.map(TargetSelector::Platform).as_ref())
                    .unwrap()
                    .dependencies
                    .get(&kind)
                    .unwrap()
                    .get(name)
                    .is_some()
            );
        }

        // Remove the dependency from the manifest
        manifest
            .remove_dependency(
                &PackageName::new_unchecked(name),
                kind,
                platforms,
                feature_name,
            )
            .unwrap();

        // The dependency should no longer exist
        for platform in to_options(platforms) {
            assert!(
                manifest
                    .workspace
                    .feature_mut(feature_name)
                    .unwrap()
                    .targets
                    .for_opt_target(platform.map(TargetSelector::Platform).as_ref())
                    .unwrap()
                    .dependencies
                    .get(&kind)
                    .unwrap()
                    .get(name)
                    .is_none()
            );
        }

        // Write the toml to string and verify the content
        assert_snapshot!(
            format!("test_remove_{}", name),
            manifest.document.to_string()
        );
    }

    #[test]
    fn test_remove_target_dependencies() {
        // Using known files in the project so the test succeed including the file
        // check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            channels = []
            platforms = ["linux-64", "win-64"]

            [dependencies]
            fooz = "*"

            [target.win-64.dependencies]
            bar = "*"

            [target.linux-64.build-dependencies]
            baz = "*"
        "#;

        test_remove(
            file_contents,
            "baz",
            SpecType::Build,
            &[Platform::Linux64],
            &FeatureName::DEFAULT,
        );
        test_remove(
            file_contents,
            "bar",
            SpecType::Run,
            &[Platform::Win64],
            &FeatureName::DEFAULT,
        );
        test_remove(
            file_contents,
            "fooz",
            SpecType::Run,
            &[],
            &FeatureName::DEFAULT,
        );
    }

    #[test]
    fn test_remove_dependencies() {
        // Using known files in the project so the test succeed including the file
        // check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            channels = []
            platforms = ["linux-64", "win-64"]

            [dependencies]
            fooz = "*"

            [target.win-64.dependencies]
            fooz = "*"

            [target.linux-64.build-dependencies]
            fooz = "*"
        "#;

        let mut manifest = parse_pixi_toml(file_contents);
        let mut manifest = manifest.editable();

        manifest
            .remove_dependency(
                &PackageName::new_unchecked("fooz"),
                SpecType::Run,
                &[],
                &FeatureName::DEFAULT,
            )
            .unwrap();

        // The dependency should be removed from the default feature
        assert!(
            manifest
                .workspace
                .default_feature()
                .targets
                .default()
                .run_dependencies()
                .map(|d| d.is_empty())
                .unwrap_or(true)
        );

        // Should still contain the fooz dependency for the different platforms
        for (platform, kind) in [
            (Platform::Linux64, SpecType::Build),
            (Platform::Win64, SpecType::Run),
        ] {
            assert!(
                manifest
                    .workspace
                    .default_feature()
                    .targets
                    .for_target(&TargetSelector::Platform(platform))
                    .unwrap()
                    .dependencies
                    .get(&kind)
                    .into_iter()
                    .flat_map(|x| x.keys())
                    .any(|x| x.as_normalized() == "fooz")
            );
        }
    }

    fn test_remove_pypi(
        file_contents: &str,
        name: &str,
        platforms: &[Platform],
        feature_name: &FeatureName,
    ) {
        let mut manifest = parse_pixi_toml(file_contents);
        let mut manifest = manifest.editable();

        let package_name = PypiPackageName::from_str(name).unwrap();

        // Initially the dependency should exist
        for platform in to_options(platforms) {
            assert!(
                manifest
                    .workspace
                    .feature_mut(feature_name)
                    .unwrap()
                    .targets
                    .for_opt_target(platform.map(TargetSelector::Platform).as_ref())
                    .unwrap()
                    .pypi_dependencies
                    .as_ref()
                    .unwrap()
                    .get(&package_name)
                    .is_some()
            );
        }

        // Remove the dependency from the manifest
        manifest
            .remove_pypi_dependency(&package_name, platforms, feature_name)
            .unwrap();

        // The dependency should no longer exist
        for platform in to_options(platforms) {
            assert!(
                manifest
                    .workspace
                    .feature_mut(feature_name)
                    .unwrap()
                    .targets
                    .for_opt_target(platform.map(TargetSelector::Platform).as_ref())
                    .unwrap()
                    .pypi_dependencies
                    .as_ref()
                    .unwrap()
                    .get(&package_name)
                    .is_none()
            );
        }

        // Write the toml to string and verify the content
        assert_snapshot!(
            format!("test_remove_pypi_{}", name),
            manifest.document.to_string()
        );
    }

    #[rstest]
    #[case::xpackage("xpackage", & [Platform::Linux64], FeatureName::default())]
    #[case::jax("jax", & [Platform::Win64], FeatureName::default())]
    #[case::requests("requests", & [], FeatureName::default())]
    #[case::feature_dep("feature_dep", & [], FeatureName::from("test"))]
    #[case::feature_target_dep(
        "feature_target_dep", & [Platform::Linux64], FeatureName::from("test")
    )]
    fn test_remove_pypi_dependencies(
        #[case] package_name: &str,
        #[case] platforms: &[Platform],
        #[case] feature_name: FeatureName,
    ) {
        let pixi_cfg = r#"[project]
name = "pixi_fun"
version = "0.1.0"
channels = []
platforms = ["linux-64", "win-64"]

[dependencies]
python = ">=3.12.1,<3.13"

[pypi-dependencies]
requests = "*"

[target.win-64.pypi-dependencies]
jax = { version = "*", extras = ["cpu"] }
requests = "*"

[target.linux-64.pypi-dependencies]
xpackage = "==1.2.3"
ypackage = {version = ">=1.2.3"}

[feature.test.pypi-dependencies]
feature_dep = "*"

[feature.test.target.linux-64.pypi-dependencies]
feature_target_dep = "*"
"#;
        test_remove_pypi(pixi_cfg, package_name, platforms, &feature_name);
    }

    #[test]
    fn test_set_version() {
        // Using known files in the project so the test succeed including the file
        // check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            channels = []
            platforms = ["linux-64", "win-64"]
        "#;

        let mut manifest = parse_pixi_toml(file_contents);
        let mut manifest = manifest.editable();

        assert_eq!(
            manifest
                .workspace
                .workspace
                .version
                .as_ref()
                .unwrap()
                .clone(),
            Version::from_str("0.1.0").unwrap()
        );

        manifest.set_version(&String::from("1.2.3")).unwrap();

        assert_eq!(
            manifest
                .workspace
                .workspace
                .version
                .as_ref()
                .unwrap()
                .clone(),
            Version::from_str("1.2.3").unwrap()
        );
    }

    #[test]
    fn test_set_description() {
        // Using known files in the project so the test succeed including the file
        // check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            description = "foo description"
            channels = []
            platforms = ["linux-64", "win-64"]
        "#;

        let mut manifest = parse_pixi_toml(file_contents);
        let mut manifest = manifest.editable();

        assert_eq!(
            manifest
                .workspace
                .workspace
                .description
                .as_ref()
                .unwrap()
                .clone(),
            String::from("foo description")
        );

        manifest
            .set_description(&String::from("my new description"))
            .unwrap();

        assert_eq!(
            manifest
                .workspace
                .workspace
                .description
                .as_ref()
                .unwrap()
                .clone(),
            String::from("my new description")
        );
    }

    #[test]
    fn test_add_platforms() {
        // Using known files in the project so the test succeed including the file
        // check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            description = "foo description"
            channels = []
            platforms = ["linux-64", "win-64"]
        "#;

        let mut manifest = parse_pixi_toml(file_contents);
        let mut manifest = manifest.editable();

        assert_eq!(
            manifest.workspace.workspace.platforms,
            vec![Platform::Linux64, Platform::Win64]
                .into_iter()
                .collect::<IndexSet<_>>()
        );

        manifest
            .add_platforms([Platform::OsxArm64].iter(), &FeatureName::DEFAULT)
            .unwrap();

        assert_eq!(
            manifest.workspace.workspace.platforms,
            vec![Platform::Linux64, Platform::Win64, Platform::OsxArm64]
                .into_iter()
                .collect::<IndexSet<_>>()
        );

        manifest
            .add_platforms(
                [Platform::LinuxAarch64, Platform::Osx64].iter(),
                &FeatureName::from("test"),
            )
            .unwrap();

        assert_eq!(
            manifest
                .workspace
                .feature(&FeatureName::from("test"))
                .unwrap()
                .platforms
                .clone()
                .unwrap(),
            vec![Platform::LinuxAarch64, Platform::Osx64]
                .into_iter()
                .collect::<IndexSet<_>>()
        );

        manifest
            .add_platforms(
                [Platform::LinuxAarch64, Platform::Win64].iter(),
                &FeatureName::from("test"),
            )
            .unwrap();

        assert_eq!(
            manifest
                .workspace
                .feature(&FeatureName::from("test"))
                .unwrap()
                .platforms
                .clone()
                .unwrap(),
            vec![Platform::LinuxAarch64, Platform::Osx64, Platform::Win64]
                .into_iter()
                .collect::<IndexSet<_>>()
        );
    }

    #[test]
    fn test_remove_platforms() {
        // Using known files in the project so the test succeed including the file
        // check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            description = "foo description"
            channels = []
            platforms = ["linux-64", "win-64"]

            [feature.test]
            platforms = ["linux-64", "win-64", "osx-64"]

            [environments]
            test = ["test"]
        "#;

        let mut manifest = parse_pixi_toml(file_contents);
        let mut manifest = manifest.editable();

        assert_eq!(
            manifest.workspace.workspace.platforms,
            vec![Platform::Linux64, Platform::Win64]
                .into_iter()
                .collect::<IndexSet<_>>()
        );

        manifest
            .remove_platforms(vec![Platform::Linux64], &FeatureName::DEFAULT)
            .unwrap();

        assert_eq!(
            manifest.workspace.workspace.platforms,
            vec![Platform::Win64].into_iter().collect::<IndexSet<_>>()
        );

        assert_eq!(
            manifest
                .workspace
                .feature(&FeatureName::from("test"))
                .unwrap()
                .platforms
                .clone()
                .unwrap(),
            vec![Platform::Linux64, Platform::Win64, Platform::Osx64]
                .into_iter()
                .collect::<IndexSet<_>>()
        );

        manifest
            .remove_platforms(
                vec![Platform::Linux64, Platform::Osx64],
                &FeatureName::from("test"),
            )
            .unwrap();

        assert_eq!(
            manifest
                .workspace
                .feature(&FeatureName::from("test"))
                .unwrap()
                .platforms
                .clone()
                .unwrap(),
            vec![Platform::Win64].into_iter().collect::<IndexSet<_>>()
        );

        // Test removing non-existing platforms
        assert!(
            manifest
                .remove_platforms(
                    vec![Platform::Linux64, Platform::Osx64],
                    &FeatureName::from("test"),
                )
                .is_err()
        );
    }

    #[test]
    fn test_add_channels() {
        // Using known files in the project so the test succeed including the file
        // check.
        let file_contents = r#"
[project]
name = "foo"
channels = []
platforms = ["linux-64", "win-64"]

[dependencies]

[feature.test.dependencies]
    "#;

        let mut manifest = parse_pixi_toml(file_contents);
        let mut manifest = manifest.editable();

        assert_eq!(manifest.workspace.workspace.channels, IndexSet::new());

        let conda_forge =
            PrioritizedChannel::from(NamedChannelOrUrl::Name(String::from("conda-forge")));
        manifest
            .add_channels([conda_forge.clone()], &FeatureName::DEFAULT, false)
            .unwrap();

        let cuda_feature = FeatureName::from("cuda");
        let nvidia = PrioritizedChannel::from(NamedChannelOrUrl::Name(String::from("nvidia")));
        manifest
            .add_channels([nvidia.clone()], &cuda_feature, false)
            .unwrap();

        let test_feature = FeatureName::from("test");
        manifest
            .add_channels(
                [
                    PrioritizedChannel::from(NamedChannelOrUrl::Name(String::from("test"))),
                    PrioritizedChannel::from(NamedChannelOrUrl::Name(String::from("test2"))),
                ],
                &test_feature,
                false,
            )
            .unwrap();

        assert_eq!(
            manifest.workspace.workspace.channels,
            vec![PrioritizedChannel {
                channel: NamedChannelOrUrl::Name(String::from("conda-forge")),
                priority: None,
            }]
            .into_iter()
            .collect::<IndexSet<_>>()
        );

        // Try to add again, should not add more channels
        manifest
            .add_channels([conda_forge.clone()], &FeatureName::DEFAULT, false)
            .unwrap();

        assert_eq!(
            manifest.workspace.workspace.channels,
            vec![PrioritizedChannel {
                channel: NamedChannelOrUrl::Name(String::from("conda-forge")),
                priority: None,
            }]
            .into_iter()
            .collect::<IndexSet<_>>()
        );

        assert_eq!(
            manifest
                .workspace
                .features
                .get(&cuda_feature)
                .unwrap()
                .channels
                .clone()
                .unwrap(),
            vec![PrioritizedChannel {
                channel: NamedChannelOrUrl::Name(String::from("nvidia")),
                priority: None,
            }]
            .into_iter()
            .collect::<IndexSet<_>>()
        );

        // Try to add again, should not add more channels
        manifest
            .add_channels([nvidia.clone()], &cuda_feature, false)
            .unwrap();

        assert_eq!(
            manifest
                .workspace
                .features
                .get(&cuda_feature)
                .unwrap()
                .channels
                .clone()
                .unwrap(),
            vec![PrioritizedChannel {
                channel: NamedChannelOrUrl::Name(String::from("nvidia")),
                priority: None,
            }]
            .into_iter()
            .collect::<IndexSet<_>>()
        );

        assert_eq!(
            manifest
                .workspace
                .features
                .get(&test_feature)
                .unwrap()
                .channels
                .clone()
                .unwrap(),
            vec![
                PrioritizedChannel {
                    channel: NamedChannelOrUrl::Name(String::from("test")),
                    priority: None,
                },
                PrioritizedChannel {
                    channel: NamedChannelOrUrl::Name(String::from("test2")),
                    priority: None,
                },
            ]
            .into_iter()
            .collect::<IndexSet<_>>()
        );

        // Test custom channel urls
        let custom_channel = PrioritizedChannel {
            channel: NamedChannelOrUrl::Url("https://custom.com/channel".parse().unwrap()),
            priority: None,
        };
        manifest
            .add_channels([custom_channel.clone()], &FeatureName::DEFAULT, false)
            .unwrap();

        assert!(
            manifest
                .workspace
                .workspace
                .channels
                .iter()
                .any(|c| c.channel == custom_channel.channel)
        );

        // Test adding priority
        let prioritized_channel1 = PrioritizedChannel {
            channel: NamedChannelOrUrl::Name(String::from("prioritized")),
            priority: Some(12i32),
        };
        manifest
            .add_channels([prioritized_channel1.clone()], &FeatureName::DEFAULT, false)
            .unwrap();

        assert!(
            manifest
                .workspace
                .workspace
                .channels
                .iter()
                .any(|c| c.channel == prioritized_channel1.channel && c.priority == Some(12i32))
        );

        let prioritized_channel2 = PrioritizedChannel {
            channel: NamedChannelOrUrl::Name(String::from("prioritized2")),
            priority: Some(-12i32),
        };
        manifest
            .add_channels([prioritized_channel2.clone()], &FeatureName::DEFAULT, false)
            .unwrap();

        assert!(
            manifest
                .workspace
                .workspace
                .channels
                .iter()
                .any(|c| c.channel == prioritized_channel2.channel && c.priority == Some(-12i32))
        );

        assert_snapshot!(manifest.document.to_string());
    }

    #[test]
    fn test_remove_channels() {
        // Using known files in the project so the test succeed including the file
        // check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            description = "foo description"
            channels = ["conda-forge"]
            platforms = ["linux-64", "win-64"]

            [feature.test]
            channels = ["test_channel"]
        "#;

        let mut manifest = parse_pixi_toml(file_contents);
        let mut manifest = manifest.editable();

        assert_eq!(
            manifest.workspace.workspace.channels,
            vec![PrioritizedChannel::from(NamedChannelOrUrl::Name(
                String::from("conda-forge")
            ))]
            .into_iter()
            .collect::<IndexSet<_>>()
        );

        manifest
            .remove_channels(
                [PrioritizedChannel {
                    channel: NamedChannelOrUrl::Name(String::from("conda-forge")),
                    priority: None,
                }],
                &FeatureName::DEFAULT,
            )
            .unwrap();

        assert_eq!(manifest.workspace.workspace.channels, IndexSet::new());

        manifest
            .remove_channels(
                [PrioritizedChannel {
                    channel: NamedChannelOrUrl::Name(String::from("test_channel")),
                    priority: None,
                }],
                &FeatureName::from("test"),
            )
            .unwrap();

        let feature_channels = manifest
            .workspace
            .feature(&FeatureName::from("test"))
            .unwrap()
            .channels
            .clone()
            .unwrap();
        assert_eq!(feature_channels, IndexSet::new());

        // Test failing to remove a channel that does not exist
        assert!(
            manifest
                .remove_channels(
                    [PrioritizedChannel {
                        channel: NamedChannelOrUrl::Name(String::from("conda-forge")),
                        priority: None,
                    }],
                    &FeatureName::DEFAULT,
                )
                .is_err()
        );
    }

    #[test]
    fn test_environments_definition() {
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            channels = ["conda-forge"]
            platforms = ["linux-64", "win-64"]

            [feature.py39.dependencies]
            python = "~=3.9.0"

            [feature.py310.dependencies]
            python = "~=3.10.0"

            [feature.cuda.dependencies]
            cudatoolkit = ">=11.0,<12.0"

            [feature.test.dependencies]
            pytest = "*"

            [environments]
            default = ["py39"]
            standard = { solve-group = "test" }
            cuda = ["cuda", "py310"]
            test1 = {features = ["test", "py310"], solve-group = "test"}
            test2 = {features = ["py39"], solve-group = "test"}
        "#;
        let manifest = parse_pixi_toml(file_contents).manifest;
        let default_env = manifest.default_environment();
        assert_eq!(default_env.name, EnvironmentName::Default);
        assert_eq!(default_env.features, vec!["py39"]);

        let cuda_env = manifest
            .environment(&EnvironmentName::Named("cuda".to_string()))
            .unwrap();
        assert_eq!(cuda_env.features, vec!["cuda", "py310"]);
        assert_eq!(cuda_env.solve_group, None);

        let test1_env = manifest
            .environment(&EnvironmentName::Named("test1".to_string()))
            .unwrap();
        assert_eq!(test1_env.features, vec!["test", "py310"]);
        assert_eq!(
            test1_env
                .solve_group
                .map(|idx| manifest.solve_groups[idx].name.as_str()),
            Some("test")
        );

        let test2_env = manifest
            .environment(&EnvironmentName::Named("test2".to_string()))
            .unwrap();
        assert_eq!(test2_env.features, vec!["py39"]);
        assert_eq!(
            test2_env
                .solve_group
                .map(|idx| manifest.solve_groups[idx].name.as_str()),
            Some("test")
        );

        assert_eq!(
            test1_env.solve_group, test2_env.solve_group,
            "both environments should share the same solve group"
        );
    }

    #[test]
    fn test_feature_definition() {
        let file_contents = r#"
            [project]
            name = "foo"
            channels = []
            platforms = []

            [feature.cuda]
            dependencies = {cuda = "x.y.z", cudnn = "12.0"}
            pypi-dependencies = {torch = "~=1.9.0"}
            build-dependencies = {cmake = "*"}
            platforms = ["linux-64", "osx-arm64"]
            activation = {scripts = ["cuda_activation.sh"]}
            system-requirements = {cuda = "12"}
            channels = ["pytorch", {channel = "nvidia", priority = -1}]
            tasks = { warmup = "python warmup.py" }
            target.osx-arm64 = {dependencies = {mlx = "x.y.z"}}

        "#;
        let manifest = parse_pixi_toml(file_contents).manifest;

        let cuda_feature = manifest.features.get(&FeatureName::from("cuda")).unwrap();
        assert_eq!(cuda_feature.name, FeatureName::from("cuda"));
        assert_eq!(
            cuda_feature
                .targets
                .default()
                .dependencies
                .get(&SpecType::Run)
                .unwrap()
                .get(&PackageName::from_str("cuda").unwrap())
                .unwrap()
                .as_version_spec(),
            Some(&VersionSpec::from_str("x.y.z", Lenient).unwrap())
        );
        assert_eq!(
            cuda_feature
                .targets
                .default()
                .dependencies
                .get(&SpecType::Run)
                .unwrap()
                .get(&PackageName::from_str("cudnn").unwrap())
                .unwrap()
                .as_version_spec(),
            Some(&VersionSpec::from_str("12", Lenient).unwrap())
        );
        assert_eq!(
            cuda_feature
                .targets
                .default()
                .pypi_dependencies
                .as_ref()
                .unwrap()
                .get(&PypiPackageName::from_str("torch").expect("torch should be a valid name"))
                .expect("pypi requirement should be available")
                .clone()
                .to_string(),
            "\"~=1.9.0\""
        );
        assert_eq!(
            cuda_feature
                .targets
                .default()
                .dependencies
                .get(&SpecType::Build)
                .unwrap()
                .get(&PackageName::from_str("cmake").unwrap())
                .unwrap()
                .as_version_spec(),
            Some(&VersionSpec::Any)
        );
        assert_eq!(
            cuda_feature
                .targets
                .default()
                .activation
                .as_ref()
                .unwrap()
                .scripts
                .as_ref()
                .unwrap(),
            &vec![String::from("cuda_activation.sh")]
        );
        assert_eq!(
            cuda_feature
                .system_requirements
                .cuda
                .as_ref()
                .unwrap()
                .to_string(),
            "12"
        );
        assert_eq!(
            cuda_feature
                .channels
                .as_ref()
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![
                &PrioritizedChannel {
                    channel: NamedChannelOrUrl::Name(String::from("pytorch")),
                    priority: None,
                },
                &PrioritizedChannel {
                    channel: NamedChannelOrUrl::Name(String::from("nvidia")),
                    priority: Some(-1),
                },
            ]
        );
        assert_eq!(
            cuda_feature
                .targets
                .for_target(&TargetSelector::Platform(Platform::OsxArm64))
                .unwrap()
                .dependencies
                .get(&SpecType::Run)
                .unwrap()
                .get(&PackageName::from_str("mlx").unwrap())
                .unwrap()
                .as_version_spec(),
            Some(&VersionSpec::from_str("x.y.z", Lenient).unwrap())
        );
        assert_eq!(
            cuda_feature
                .targets
                .default()
                .tasks
                .get(&"warmup".into())
                .unwrap()
                .as_single_command(None)
                .unwrap()
                .unwrap(),
            "python warmup.py"
        );
    }

    #[rstest]
    #[case::empty("", false)]
    #[case::just_dependencies("[dependencies]", false)]
    #[case::with_pypi_dependencies("[pypi-dependencies]\nfoo=\"*\"", true)]
    #[case::empty_pypi_dependencies("[pypi-dependencies]", true)]
    #[case::nested_in_feature_and_target("[feature.foo.target.linux-64.pypi-dependencies]", true)]
    fn test_has_pypi_dependencies(
        #[case] file_contents: &str,
        #[case] should_have_pypi_dependencies: bool,
    ) {
        let manifest =
            parse_pixi_toml(format!("{PROJECT_BOILERPLATE}\n{file_contents}").as_str()).manifest;

        assert_eq!(
            manifest.has_pypi_dependencies(),
            should_have_pypi_dependencies,
        );
    }

    #[test]
    fn test_add_task() {
        let file_contents = r#"
[project]
name = "foo"
version = "0.1.0"
description = "foo description"
channels = []
platforms = ["linux-64", "win-64"]

[tasks]
test = "test initial"

        "#;

        let mut manifest = parse_pixi_toml(file_contents);
        let mut manifest = manifest.editable();

        manifest
            .add_task(
                "default".into(),
                Task::Plain("echo default".into()),
                None,
                &FeatureName::DEFAULT,
            )
            .unwrap();
        manifest
            .add_task(
                "target_linux".into(),
                Task::Plain("echo target_linux".into()),
                Some(Platform::Linux64),
                &FeatureName::DEFAULT,
            )
            .unwrap();
        manifest
            .add_task(
                "feature_test".into(),
                Task::Plain("echo feature_test".into()),
                None,
                &FeatureName::from("test"),
            )
            .unwrap();
        manifest
            .add_task(
                "feature_test_target_linux".into(),
                Task::Plain("echo feature_test_target_linux".into()),
                Some(Platform::Linux64),
                &FeatureName::from("test"),
            )
            .unwrap();
        assert_snapshot!(manifest.document.to_string());
    }

    #[test]
    fn test_add_dependency() {
        let file_contents = r#"
[project]
name = "foo"
channels = []
platforms = ["linux-64", "win-64"]

[dependencies]
foo = "*"

[feature.test.dependencies]
bar = "*"
            "#;
        let channel_config = default_channel_config();
        let mut manifest = parse_pixi_toml(file_contents);
        let mut manifest = manifest.editable();

        // Determine the name of the package to add
        let spec = &MatchSpec::from_str("baz >=1.2.3", Strict).unwrap();

        let (name, spec) = spec.clone().into_nameless();
        let name = name.unwrap();

        let spec = PixiSpec::from_nameless_matchspec(spec, &channel_config);

        manifest
            .add_dependency(
                &name,
                &spec,
                SpecType::Run,
                &[],
                &FeatureName::DEFAULT,
                DependencyOverwriteBehavior::Overwrite,
            )
            .unwrap();
        assert_eq!(
            manifest
                .workspace
                .default_feature()
                .targets
                .default()
                .dependencies
                .get(&SpecType::Run)
                .unwrap()
                .get(&PackageName::from_str("baz").unwrap())
                .unwrap()
                .as_version_spec(),
            Some(&VersionSpec::from_str(">=1.2.3", Strict).unwrap())
        );

        let (name, spec) = MatchSpec::from_str("bal >=2.3", Strict)
            .unwrap()
            .into_nameless();
        let pixi_spec = PixiSpec::from_nameless_matchspec(spec, &channel_config);

        manifest
            .add_dependency(
                &name.unwrap(),
                &pixi_spec,
                SpecType::Run,
                &[],
                &FeatureName::from("test"),
                DependencyOverwriteBehavior::Overwrite,
            )
            .unwrap();

        assert_eq!(
            manifest
                .workspace
                .feature(&FeatureName::from("test"))
                .unwrap()
                .targets
                .default()
                .dependencies
                .get(&SpecType::Run)
                .unwrap()
                .get(&PackageName::from_str("bal").unwrap())
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            ">=2.3".to_string()
        );

        let (package_name, nameless) = MatchSpec::from_str(" boef >=2.3", Strict)
            .unwrap()
            .into_nameless();
        let pixi_spec = PixiSpec::from_nameless_matchspec(nameless, &channel_config);

        manifest
            .add_dependency(
                &package_name.unwrap(),
                &pixi_spec,
                SpecType::Run,
                &[Platform::Linux64],
                &FeatureName::from("extra"),
                DependencyOverwriteBehavior::Overwrite,
            )
            .unwrap();

        assert_eq!(
            manifest
                .workspace
                .feature(&FeatureName::from("extra"))
                .unwrap()
                .targets
                .for_target(&TargetSelector::Platform(Platform::Linux64))
                .unwrap()
                .dependencies
                .get(&SpecType::Run)
                .unwrap()
                .get(&PackageName::from_str("boef").unwrap())
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            ">=2.3".to_string()
        );

        let matchspec = MatchSpec::from_str(" cmake >=2.3", ParseStrictness::Strict).unwrap();
        let (package_name, nameless) = matchspec.into_nameless();

        let pixi_spec = PixiSpec::from_nameless_matchspec(nameless, &channel_config);

        manifest
            .add_dependency(
                &package_name.unwrap(),
                &pixi_spec,
                SpecType::Build,
                &[Platform::Linux64],
                &FeatureName::from("build"),
                DependencyOverwriteBehavior::Overwrite,
            )
            .unwrap();

        assert_eq!(
            manifest
                .workspace
                .feature(&FeatureName::from("build"))
                .map(|f| &f.targets)
                .and_then(|t| t.for_target(&TargetSelector::Platform(Platform::Linux64)))
                .and_then(|t| t.dependencies.get(&SpecType::Build))
                .and_then(|deps| deps.get(&PackageName::from_str("cmake").unwrap()))
                .and_then(|spec| spec.as_version_spec())
                .map(|spec| spec.to_string())
                .unwrap(),
            ">=2.3".to_string()
        );

        assert_snapshot!(manifest.document.to_string());
    }

    #[test]
    fn test_add_environment() {
        let contents = r#"
        [project]
        name = "foo"
        channels = []
        platforms = []

        [environments]
        "#;
        let mut manifest = parse_pixi_toml(contents);
        let mut manifest = manifest.editable();

        manifest
            .add_environment(String::from("test"), Some(Vec::new()), None, false)
            .unwrap();
        assert!(manifest.workspace.environment("test").is_some());
    }

    #[test]
    fn test_add_environment_with_feature() {
        let contents = r#"
        [project]
        name = "foo"
        channels = []
        platforms = []

        [feature.foobar]

        [environments]
        "#;
        let mut manifest = parse_pixi_toml(contents);
        let mut manifest = manifest.editable();

        manifest
            .add_environment(
                String::from("test"),
                Some(vec![String::from("foobar")]),
                None,
                false,
            )
            .unwrap();
        assert!(manifest.workspace.environment("test").is_some());
    }

    #[test]
    fn test_add_environment_non_existing_feature() {
        let contents = r#"
        [project]
        name = "foo"
        channels = []
        platforms = []

        [feature.existing]

        [environments]
        "#;
        let mut manifest = parse_pixi_toml(contents);
        let mut manifest = manifest.editable();

        let err = manifest
            .add_environment(
                String::from("test"),
                Some(vec![String::from("non-existing")]),
                None,
                false,
            )
            .unwrap_err();

        // Disable colors in tests
        let mut s = String::new();
        let report_handler = NarratableReportHandler::new().with_cause_chain();
        report_handler.render_report(&mut s, err.as_ref()).unwrap();

        assert_snapshot!(s, @r###"
        the feature 'non-existing' is not defined in the project manifest
            Diagnostic severity: error
        diagnostic help: Did you mean 'existing'?
        "###);
    }

    #[test]
    fn test_remove_environment() {
        let contents = r#"
        [project]
        name = "foo"
        channels = []
        platforms = []

        [environments]
        foo = []
        "#;
        let mut manifest = parse_pixi_toml(contents);
        let mut manifest = manifest.editable();

        assert!(manifest.remove_environment("foo").unwrap());
        assert!(!manifest.remove_environment("default").unwrap());
    }

    #[test]
    pub fn test_channel_priority_manifest() {
        let contents = r#"
        [project]
        name = "foo"
        platforms = []
        channels = []

        [feature.strict]
        channel-priority = "strict"

        [feature.disabled]
        channel-priority = "disabled"

        [environments]
        test-strict = ["strict"]
        test-disabled = ["disabled"]
        "#;

        let manifest = parse_pixi_toml(contents).manifest;

        assert!(manifest.default_feature().channel_priority.is_none());
        assert_eq!(
            manifest
                .feature("strict")
                .unwrap()
                .channel_priority
                .unwrap(),
            ChannelPriority::Strict
        );
        assert_eq!(
            manifest
                .feature("disabled")
                .unwrap()
                .channel_priority
                .unwrap(),
            ChannelPriority::Disabled
        );

        let contents = r#"
        [project]
        name = "foo"
        platforms = []
        channels = []
        channel-priority = "disabled"
        "#;

        let manifest = parse_pixi_toml(contents).manifest;

        assert_eq!(
            manifest.default_feature().channel_priority.unwrap(),
            ChannelPriority::Disabled
        );
    }

    #[test]
    fn test_prepend_channels() {
        let contents = r#"
            [project]
            name = "foo"
            channels = ["conda-forge"]
            platforms = []
        "#;
        let mut manifest = parse_pixi_toml(contents);
        let mut manifest = manifest.editable();

        // Add pytorch channel with prepend=true
        let pytorch = PrioritizedChannel::from(NamedChannelOrUrl::Name(String::from("pytorch")));
        manifest
            .add_channels([pytorch.clone()], &FeatureName::DEFAULT, true)
            .unwrap();

        // Verify pytorch is first in the list
        assert_eq!(
            manifest
                .workspace
                .workspace
                .channels
                .iter()
                .next()
                .unwrap()
                .channel,
            pytorch.channel
        );

        // Add another channel without prepend
        let bioconda = PrioritizedChannel::from(NamedChannelOrUrl::Name(String::from("bioconda")));
        manifest
            .add_channels([bioconda.clone()], &FeatureName::DEFAULT, false)
            .unwrap();

        // Verify order is still pytorch, conda-forge, bioconda
        let channels: Vec<_> = manifest
            .workspace
            .workspace
            .channels
            .iter()
            .map(|c| c.channel.to_string())
            .collect();
        assert_eq!(channels, vec!["pytorch", "conda-forge", "bioconda"]);
    }

    #[test]
    fn test_validation_failure_source_dependency() {
        let toml = r#"
        [project]
        name = "test"
        channels = ['conda-forge']
        platforms = ['linux-64']

        [dependencies]
        foo = { path = "./foo" }
        "#;

        let manifest = WorkspaceManifest::from_toml_str(toml);
        let err = manifest.unwrap_err();
        insta::assert_snapshot!(format_parse_error(toml, err.error), @r###"
         × conda source dependencies are not allowed without enabling the 'pixi-build' preview feature
          ╭─[pixi.toml:8:15]
        7 │         [dependencies]
        8 │         foo = { path = "./foo" }
          ·               ─────────┬────────
          ·                        ╰── source dependency specified here
        9 │
          ╰────
         help: Add `preview = ["pixi-build"]` to the `workspace` or `project` table of your manifest
        "###);
    }

    #[test]
    fn test_platform_remove() {
        let toml = r#"
        [workspace]
        name = "test"
        channels = ['conda-forge']
        platforms = ['linux-64', 'win-64']
        "#;

        let mut manifest = parse_pixi_toml(toml);
        let mut manifest = manifest.editable();

        manifest
            .remove_platforms([Platform::Linux64], &FeatureName::DEFAULT)
            .unwrap();

        assert_snapshot!(manifest.document.to_string(), @r###"
        [workspace]
        name = "test"
        channels = ['conda-forge']
        platforms = [ 'win-64']
        "###);
    }

    #[test]
    fn test_requires_pixi() {
        let contents = r#"
        [project]
        name = "foo"
        channels = []
        platforms = []
        requires-pixi = "==0.1"
        "#;
        let manifest = parse_pixi_toml(contents).manifest;

        assert_eq!(
            manifest.workspace.requires_pixi,
            VersionSpec::from_str("==0.1.0", Lenient).ok()
        );

        let contents_no = r#"
        [project]
        name = "foo"
        channels = []
        platforms = []
        "#;
        let manifest_no = parse_pixi_toml(contents_no).manifest;
        assert_eq!(manifest_no.workspace.requires_pixi, None);
    }
}
