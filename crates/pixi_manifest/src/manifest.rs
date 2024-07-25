use crate::document::ManifestSource;
use crate::error::{DependencyError, TomlError, UnknownFeature};
use crate::pypi::PyPiPackageName;
use crate::pyproject::PyProjectManifest;
use crate::{
    consts, to_options, DependencyOverwriteBehavior, Environment, EnvironmentName, Feature,
    FeatureName, GetFeatureError, ParsedManifest, PrioritizedChannel, SolveGroup, SpecType, Target,
    TargetSelector, Task, TaskName,
};
use indexmap::{Equivalent, IndexSet};
use itertools::Itertools;
use miette::{miette, IntoDiagnostic, NamedSource, WrapErr};
use rattler_conda_types::{ChannelConfig, MatchSpec, PackageName, Platform, Version};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt::Display;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use toml_edit::DocumentMut;

#[derive(Debug, Clone)]
pub enum ManifestKind {
    Pixi,
    Pyproject,
}

impl ManifestKind {
    /// Try to determine the type of manifest from a path
    pub fn try_from_path(path: &Path) -> Option<Self> {
        match path.file_name().and_then(OsStr::to_str)? {
            consts::PROJECT_MANIFEST => Some(Self::Pixi),
            consts::PYPROJECT_MANIFEST => Some(Self::Pyproject),
            _ => None,
        }
    }
}

/// Handles the project's manifest file.
/// This struct is responsible for reading, parsing, editing, and saving the
/// manifest. It encapsulates all logic related to the manifest's TOML format
/// and structure. The manifest data is represented as a [`ParsedManifest`]
/// struct for easy manipulation.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The path to the manifest file
    pub path: PathBuf,

    /// The raw contents of the manifest file
    pub contents: String,

    /// Editable toml document
    pub document: ManifestSource,

    /// The parsed manifest
    pub parsed: ParsedManifest,
}

impl Borrow<ParsedManifest> for Manifest {
    fn borrow(&self) -> &ParsedManifest {
        &self.parsed
    }
}

impl Manifest {
    /// Create a new manifest from a path
    pub fn from_path(path: impl AsRef<Path>) -> miette::Result<Self> {
        let contents = std::fs::read_to_string(path.as_ref()).into_diagnostic()?;
        Self::from_str(path.as_ref(), contents)
    }

    /// Return the toml manifest file name ('pixi.toml' or 'pyproject.toml')
    pub fn file_name(&self) -> &str {
        match self.document {
            ManifestSource::PixiToml(_) => consts::PROJECT_MANIFEST,
            ManifestSource::PyProjectToml(_) => consts::PYPROJECT_MANIFEST,
        }
    }

    /// Create a new manifest from a string
    pub fn from_str(manifest_path: &Path, contents: impl Into<String>) -> miette::Result<Self> {
        let manifest_kind = ManifestKind::try_from_path(manifest_path).ok_or_else(|| {
            miette::miette!("unrecognized manifest file: {}", manifest_path.display())
        })?;
        let root = manifest_path
            .parent()
            .expect("manifest_path should always have a parent");

        let contents = contents.into();
        let (parsed, file_name) = match manifest_kind {
            ManifestKind::Pixi => (ParsedManifest::from_toml_str(&contents), "pixi.toml"),
            ManifestKind::Pyproject => (
                PyProjectManifest::from_toml_str(&contents).map(|x| x.into()),
                "pyproject.toml",
            ),
        };

        let (manifest, document) = match parsed.and_then(|manifest| {
            contents
                .parse::<DocumentMut>()
                .map(|doc| (manifest, doc))
                .map_err(TomlError::from)
        }) {
            Ok(result) => result,
            Err(e) => e.to_fancy(file_name, &contents)?,
        };

        // Validate the contents of the manifest
        manifest.validate(NamedSource::new(file_name, contents.to_owned()), root)?;

        let source = match manifest_kind {
            ManifestKind::Pixi => ManifestSource::PixiToml(document),
            ManifestKind::Pyproject => ManifestSource::PyProjectToml(document),
        };

        Ok(Self {
            path: manifest_path.to_path_buf(),
            contents,
            document: source,
            parsed: manifest,
        })
    }

    /// Save the manifest to the file and update the contents
    pub fn save(&mut self) -> miette::Result<()> {
        self.contents = self.document.to_string();
        std::fs::write(&self.path, self.contents.clone()).into_diagnostic()?;
        Ok(())
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
            .feature(feature_name)
            // Return error if feature does not exist
            .ok_or(GetFeatureError::FeatureDoesNotExist(feature_name.clone()))?
            .targets
            .resolve(platform)
            .collect_vec()
            .into_iter()
            .rev()
            .flat_map(|target| target.tasks.iter())
            .map(|(name, task)| (name.clone(), task))
            .collect())
    }

    /// Add a task to the project
    pub fn add_task(
        &mut self,
        name: TaskName,
        task: Task,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Check if the task already exists
        if let Ok(tasks) = self.tasks(platform, feature_name) {
            if tasks.contains_key(&name) {
                miette::bail!("task {} already exists", name);
            }
        }

        // Add the task to the Toml manifest
        self.document
            .add_task(name.as_str(), task.clone(), platform, feature_name)?;

        // Add the task to the manifest
        self.get_or_insert_target_mut(platform, Some(feature_name))
            .tasks
            .insert(name, task);

        Ok(())
    }

    /// Adds an environment to the project. Overwrites the entry if it already
    /// exists.
    pub fn add_environment(
        &mut self,
        name: String,
        features: Option<Vec<String>>,
        solve_group: Option<String>,
        no_default_feature: bool,
    ) -> miette::Result<()> {
        // Make sure the features exist
        for feature in features.iter().flatten() {
            if self.feature(feature.as_str()).is_none() {
                return Err(UnknownFeature::new(feature.to_string(), &self.parsed).into());
            }
        }

        self.document.add_environment(
            name.clone(),
            features.clone(),
            solve_group.clone(),
            no_default_feature,
        )?;

        let environment_idx = self.parsed.environments.add(Environment {
            name: EnvironmentName::Named(name),
            features: features.unwrap_or_default(),
            features_source_loc: None,
            solve_group: None,
            no_default_feature,
        });

        if let Some(solve_group) = solve_group {
            self.parsed.solve_groups.add(solve_group, environment_idx);
        }

        Ok(())
    }

    /// Removes an environment from the project.
    pub fn remove_environment(&mut self, name: &str) -> miette::Result<bool> {
        // Remove the environment from the TOML document
        if !self.document.remove_environment(name)? {
            return Ok(false);
        }

        // Remove the environment from the internal manifest
        let environment_idx = self
            .parsed
            .environments
            .by_name
            .shift_remove(name)
            .expect("environment should exist");

        // Remove the environment from the solve groups
        self.parsed
            .solve_groups
            .iter_mut()
            .for_each(|group| group.environments.retain(|&idx| idx != environment_idx));

        Ok(true)
    }

    /// Remove a task from the project, and the tasks that depend on it
    pub fn remove_task(
        &mut self,
        name: TaskName,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Check if the task exists
        self.tasks(platform, feature_name)?
            .get(&name)
            .ok_or_else(|| miette::miette!("task {} does not exist", name))?;

        // Remove the task from the Toml manifest
        self.document
            .remove_task(name.as_str(), platform, feature_name)?;

        // Remove the task from the internal manifest
        self.feature_mut(feature_name)?
            .targets
            .for_opt_target_mut(platform.map(TargetSelector::from).as_ref())
            .map(|target| target.tasks.remove(&name));

        Ok(())
    }

    /// Add a platform to the project
    pub fn add_platforms<'a>(
        &mut self,
        platforms: impl Iterator<Item = &'a Platform> + Clone,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Get current and new platforms for the feature
        let current = match feature_name {
            FeatureName::Default => self.parsed.project.platforms.get_mut(),
            FeatureName::Named(_) => self.get_or_insert_feature_mut(feature_name).platforms_mut(),
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
    pub fn remove_platforms(
        &mut self,
        platforms: impl IntoIterator<Item = Platform>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Get current platforms and platform to remove for the feature
        let current = match feature_name {
            FeatureName::Default => self.parsed.project.platforms.get_mut(),
            FeatureName::Named(_) => self.feature_mut(feature_name)?.platforms_mut(),
        };
        // Get the platforms to remove, while checking if they exist
        let to_remove: IndexSet<_> = platforms
            .into_iter()
            .map(|c| {
                current
                    .iter()
                    .position(|x| *x == c)
                    .ok_or_else(|| miette::miette!("platform {} does not exist", c))
                    .map(|_| c)
            })
            .collect::<Result<_, _>>()?;

        let retained: IndexSet<_> = current.difference(&to_remove).cloned().collect();

        // Remove platforms from the manifest
        current.retain(|p| retained.contains(p));

        // And from the TOML document
        let retained = retained.iter().map(|p| p.to_string()).collect_vec();
        let platforms = self.document.get_array_mut("platforms", feature_name)?;
        platforms.retain(|x| retained.contains(&x.to_string()));

        Ok(())
    }

    /// Add a matchspec to the manifest
    pub fn add_dependency(
        &mut self,
        spec: &MatchSpec,
        spec_type: SpecType,
        platforms: &[Platform],
        feature_name: &FeatureName,
        overwrite_behavior: DependencyOverwriteBehavior,
        channel_config: &ChannelConfig,
    ) -> miette::Result<bool> {
        // Determine the name of the package to add
        let (Some(name), spec) = spec.clone().into_nameless() else {
            miette::bail!("pixi does not support wildcard dependencies")
        };
        let mut any_added = false;
        for platform in to_options(platforms) {
            // Add the dependency to the manifest
            match self
                .get_or_insert_target_mut(platform, Some(feature_name))
                .try_add_dependency(&name, &spec, spec_type, overwrite_behavior)
            {
                Ok(true) => {
                    self.document.add_dependency(
                        &name,
                        &spec,
                        spec_type,
                        platform,
                        feature_name,
                        channel_config
                    )?;
                    any_added = true;
                }
                Ok(false) => {}
                Err(e) => return Err(e.into()),
            };
        }
        Ok(any_added)
    }

    /// Add a pypi requirement to the manifest
    pub fn add_pypi_dependency(
        &mut self,
        requirement: &pep508_rs::Requirement,
        platforms: &[Platform],
        feature_name: &FeatureName,
        editable: Option<bool>,
        overwrite_behavior: DependencyOverwriteBehavior,
    ) -> miette::Result<bool> {
        let mut any_added = false;
        for platform in crate::to_options(platforms) {
            // Add the pypi dependency to the manifest
            match self
                .get_or_insert_target_mut(platform, Some(feature_name))
                .try_add_pypi_dependency(requirement, editable, overwrite_behavior)
            {
                Ok(true) => {
                    self.document.add_pypi_dependency(
                        requirement,
                        platform,
                        feature_name,
                        editable,
                    )?;
                    any_added = true;
                }
                Ok(false) => {}
                Err(e) => return Err(e.into()),
            };
        }
        Ok(any_added)
    }

    /// Removes a dependency based on `SpecType`.
    pub fn remove_dependency(
        &mut self,
        dep: &PackageName,
        spec_type: SpecType,
        platforms: &[Platform],
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        for platform in crate::to_options(platforms) {
            // Remove the dependency from the manifest
            match self
                .target_mut(platform, feature_name)
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

    /// Removes a pypi dependency.
    pub fn remove_pypi_dependency(
        &mut self,
        dep: &PyPiPackageName,
        platforms: &[Platform],
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        for platform in crate::to_options(platforms) {
            // Remove the dependency from the manifest
            match self
                .target_mut(platform, feature_name)
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

    /// Returns true if any of the features has pypi dependencies defined.
    ///
    /// This also returns true if the `pypi-dependencies` key is defined but
    /// empty.
    pub fn has_pypi_dependencies(&self) -> bool {
        self.parsed
            .features
            .values()
            .flat_map(|f| f.targets.targets())
            .any(|f| f.pypi_dependencies.is_some())
    }

    /// Adds the specified channels to the manifest.
    pub fn add_channels(
        &mut self,
        channels: impl IntoIterator<Item = PrioritizedChannel>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Get current and new platforms for the feature
        let current = match feature_name {
            FeatureName::Default => &mut self.parsed.project.channels,
            FeatureName::Named(_) => self.get_or_insert_feature_mut(feature_name).channels_mut(),
        };
        let to_add: IndexSet<_> = channels.into_iter().collect();
        let new: IndexSet<_> = to_add.difference(current).cloned().collect();

        // Add the channels to the manifest
        current.extend(new.clone());

        // Then to the TOML document
        let channels = self.document.get_array_mut("channels", feature_name)?;
        for channel in new.iter() {
            channels.push(channel.channel.to_string())
        }

        Ok(())
    }

    /// Remove the specified channels to the manifest.
    pub fn remove_channels(
        &mut self,
        channels: impl IntoIterator<Item = PrioritizedChannel>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Get current channels and channels to remove for the feature
        let current = match feature_name {
            FeatureName::Default => &mut self.parsed.project.channels,
            FeatureName::Named(_) => self.feature_mut(feature_name)?.channels_mut(),
        };

        // Get the channels to remove, while checking if they exist
        let to_remove: IndexSet<_> = channels
            .into_iter()
            .map(|c| {
                current
                    .iter()
                    .position(|x| x.channel == c.channel)
                    .ok_or_else(|| miette::miette!("channel {} does not exist", c.channel.as_str()))
                    .map(|_| c)
            })
            .collect::<Result<_, _>>()?;

        let retained: IndexSet<_> = current.difference(&to_remove).cloned().collect();

        // Remove channels from the manifest
        current.retain(|c| retained.contains(c));

        // And from the TOML document
        let retained = retained.iter().map(|c| c.channel.as_str()).collect_vec();
        let channels = self.document.get_array_mut("channels", feature_name)?;
        channels.retain(|x| retained.contains(&x.as_str().unwrap()));

        Ok(())
    }

    /// Set the project description
    pub fn set_description(&mut self, description: &str) -> miette::Result<()> {
        // Update in both the manifest and the toml
        self.parsed.project.description = Some(description.to_string());
        self.document.set_description(description);

        Ok(())
    }

    /// Set the project version
    pub fn set_version(&mut self, version: &str) -> miette::Result<()> {
        // Update in both the manifest and the toml
        self.parsed.project.version = Some(
            Version::from_str(version)
                .into_diagnostic()
                .context("could not convert version to a valid project version")?,
        );
        self.document.set_version(version);
        Ok(())
    }

    /// Returns a mutable reference to a target, creating it if needed
    pub fn get_or_insert_target_mut(
        &mut self,
        platform: Option<Platform>,
        name: Option<&FeatureName>,
    ) -> &mut Target {
        let feature = match name {
            Some(feature) => self.get_or_insert_feature_mut(feature),
            None => self.default_feature_mut(),
        };
        feature
            .targets
            .for_opt_target_or_default_mut(platform.map(TargetSelector::from).as_ref())
    }

    /// Returns a mutable reference to a target
    pub fn target_mut(&mut self, platform: Option<Platform>, name: &FeatureName) -> &mut Target {
        self.feature_mut(name)
            .unwrap()
            .targets
            .for_opt_target_mut(platform.map(TargetSelector::Platform).as_ref())
            .expect("target should exist")
    }

    /// Returns the default feature.
    ///
    /// This is the feature that is added implicitly by the tables at the root
    /// of the project manifest.
    pub fn default_feature(&self) -> &Feature {
        self.parsed.default_feature()
    }

    /// Returns a mutable reference to the default feature.
    pub fn default_feature_mut(&mut self) -> &mut Feature {
        self.parsed.default_feature_mut()
    }

    /// Returns the mutable feature with the given name or `None` if it does not
    /// exist.
    pub fn feature_mut<Q: ?Sized>(&mut self, name: &Q) -> miette::Result<&mut Feature>
    where
        Q: Hash + Equivalent<FeatureName> + Display,
    {
        self.parsed
            .features
            .get_mut(name)
            .ok_or_else(|| miette!("Feature `{name}` does not exist"))
    }

    /// Returns the mutable feature with the given name or `None` if it does not
    /// exist.
    pub fn get_or_insert_feature_mut(&mut self, name: &FeatureName) -> &mut Feature {
        self.parsed
            .features
            .entry(name.clone())
            .or_insert_with(|| Feature::new(name.clone()))
    }

    /// Returns the feature with the given name or `None` if it does not exist.
    pub fn feature<Q: ?Sized>(&self, name: &Q) -> Option<&Feature>
    where
        Q: Hash + Equivalent<FeatureName>,
    {
        self.parsed.features.get(name)
    }

    /// Returns the default environment
    ///
    /// This is the environment that is added implicitly as the environment with
    /// only the default feature. The default environment can be overwritten
    /// by a environment named `default`.
    pub fn default_environment(&self) -> &Environment {
        self.parsed.default_environment()
    }

    /// Returns the environment with the given name or `None` if it does not
    /// exist.
    pub fn environment<Q: ?Sized>(&self, name: &Q) -> Option<&Environment>
    where
        Q: Hash + Equivalent<EnvironmentName>,
    {
        self.parsed.environments.find(name)
    }

    /// Returns the solve group with the given name or `None` if it does not
    /// exist.
    pub fn solve_group<Q: ?Sized>(&self, name: &Q) -> Option<&SolveGroup>
    where
        Q: Hash + Equivalent<String>,
    {
        self.parsed.solve_groups.find(name)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use indexmap::IndexMap;
    use insta::{assert_snapshot};
    use miette::NarratableReportHandler;
    use rattler_conda_types::{ChannelConfig, NamedChannelOrUrl, ParseStrictness};
    use rattler_conda_types::ParseStrictness::Strict;
    use rattler_solve::ChannelPriority;
    use rstest::*;
    use tempfile::tempdir;

    use super::*;
    use crate::manifest::Manifest;
    use crate::{channel::PrioritizedChannel, utils::default_channel_config};

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = ["linux-64", "win-64", "osx-64"]
        "#;

    #[test]
    fn test_from_path() {
        // Test the toml from a path
        let dir = tempdir().unwrap();
        let path = dir.path().join("pixi.toml");
        std::fs::write(&path, PROJECT_BOILERPLATE).unwrap();
        // From &PathBuf
        let _manifest = Manifest::from_path(&path).unwrap();
        // From &Path
        let _manifest = Manifest::from_path(path.as_path()).unwrap();
        // From PathBuf
        let manifest = Manifest::from_path(path).unwrap();

        assert_eq!(manifest.parsed.project.name.unwrap(), "foo");
        assert_eq!(
            manifest.parsed.project.version,
            Some(Version::from_str("0.1.0").unwrap())
        );
    }

    #[test]
    fn test_activation_scripts() {
        let contents = r#"
            [project]
            name = "foo"
            channels = []
            platforms = ["win-64", "linux-64"]

            [activation]
            scripts = [".pixi/install/setup.sh"]

            [target.win-64.activation]
            scripts = [".pixi/install/setup.ps1"]

            [target.linux-64.activation]
            scripts = [".pixi/install/setup.sh", "test"]
            "#;

        let manifest = Manifest::from_str(Path::new("pixi.toml"), contents).unwrap();
        let default_activation_scripts = manifest
            .default_feature()
            .targets
            .default()
            .activation
            .as_ref()
            .and_then(|a| a.scripts.as_ref());
        let win64_activation_scripts = manifest
            .default_feature()
            .targets
            .for_target(&TargetSelector::Platform(Platform::Win64))
            .unwrap()
            .activation
            .as_ref()
            .and_then(|a| a.scripts.as_ref());
        let linux64_activation_scripts = manifest
            .default_feature()
            .targets
            .for_target(&TargetSelector::Platform(Platform::Linux64))
            .unwrap()
            .activation
            .as_ref()
            .and_then(|a| a.scripts.as_ref());

        assert_eq!(
            default_activation_scripts,
            Some(&vec![String::from(".pixi/install/setup.sh")])
        );
        assert_eq!(
            win64_activation_scripts,
            Some(&vec![String::from(".pixi/install/setup.ps1")])
        );
        assert_eq!(
            linux64_activation_scripts,
            Some(&vec![
                String::from(".pixi/install/setup.sh"),
                String::from("test"),
            ])
        );
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

        let manifest = Manifest::from_str(Path::new("pixi.toml"), contents).unwrap();
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
        let feature_targets = &manifest
            .feature(&FeatureName::Named(String::from("bar")))
            .unwrap()
            .targets;
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
        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        // Initially the dependency should exist
        for platform in to_options(platforms) {
            assert!(manifest
                .feature_mut(feature_name)
                .unwrap()
                .targets
                .for_opt_target(platform.map(TargetSelector::Platform).as_ref())
                .unwrap()
                .dependencies
                .get(&kind)
                .unwrap()
                .get(name)
                .is_some());
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
            assert!(manifest
                .feature_mut(feature_name)
                .unwrap()
                .targets
                .for_opt_target(platform.map(TargetSelector::Platform).as_ref())
                .unwrap()
                .dependencies
                .get(&kind)
                .unwrap()
                .get(name)
                .is_none());
        }

        // Write the toml to string and verify the content
        assert_snapshot!(
            format!("test_remove_{}", name),
            manifest.document.to_string()
        );
    }

    fn test_remove_pypi(
        file_contents: &str,
        name: &str,
        platforms: &[Platform],
        feature_name: &FeatureName,
    ) {
        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        let package_name = PyPiPackageName::from_str(name).unwrap();

        // Initially the dependency should exist
        for platform in to_options(platforms) {
            assert!(manifest
                .feature_mut(feature_name)
                .unwrap()
                .targets
                .for_opt_target(platform.map(TargetSelector::Platform).as_ref())
                .unwrap()
                .pypi_dependencies
                .as_ref()
                .unwrap()
                .get(&package_name)
                .is_some());
        }

        // Remove the dependency from the manifest
        manifest
            .remove_pypi_dependency(&package_name, platforms, feature_name)
            .unwrap();

        // The dependency should no longer exist
        for platform in to_options(platforms) {
            assert!(manifest
                .feature_mut(feature_name)
                .unwrap()
                .targets
                .for_opt_target(platform.map(TargetSelector::Platform).as_ref())
                .unwrap()
                .pypi_dependencies
                .as_ref()
                .unwrap()
                .get(&package_name)
                .is_none());
        }

        // Write the toml to string and verify the content
        assert_snapshot!(
            format!("test_remove_pypi_{}", name),
            manifest.document.to_string()
        );
    }

    #[rstest]
    #[case::xpackage("xpackage", & [Platform::Linux64], FeatureName::Default)]
    #[case::jax("jax", & [Platform::Win64], FeatureName::Default)]
    #[case::requests("requests", & [], FeatureName::Default)]
    #[case::feature_dep("feature_dep", & [], FeatureName::Named("test".to_string()))]
    #[case::feature_target_dep(
        "feature_target_dep", & [Platform::Linux64], FeatureName::Named("test".to_string())
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
            &FeatureName::Default,
        );
        test_remove(
            file_contents,
            "bar",
            SpecType::Run,
            &[Platform::Win64],
            &FeatureName::Default,
        );
        test_remove(
            file_contents,
            "fooz",
            SpecType::Run,
            &[],
            &FeatureName::Default,
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

        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        manifest
            .remove_dependency(
                &PackageName::new_unchecked("fooz"),
                SpecType::Run,
                &[],
                &FeatureName::Default,
            )
            .unwrap();

        // The dependency should be removed from the default feature
        assert!(manifest
            .default_feature()
            .targets
            .default()
            .run_dependencies()
            .map(|d| d.is_empty())
            .unwrap_or(true));

        // Should still contain the fooz dependency for the different platforms
        for (platform, kind) in [
            (Platform::Linux64, SpecType::Build),
            (Platform::Win64, SpecType::Run),
        ] {
            assert!(manifest
                .default_feature()
                .targets
                .for_target(&TargetSelector::Platform(platform))
                .unwrap()
                .dependencies
                .get(&kind)
                .into_iter()
                .flat_map(|x| x.keys())
                .any(|x| x.as_normalized() == "fooz"));
        }
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

        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        assert_eq!(
            manifest.parsed.project.version.as_ref().unwrap().clone(),
            Version::from_str("0.1.0").unwrap()
        );

        manifest.set_version(&String::from("1.2.3")).unwrap();

        assert_eq!(
            manifest.parsed.project.version.as_ref().unwrap().clone(),
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

        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        assert_eq!(
            manifest
                .parsed
                .project
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
                .parsed
                .project
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

        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        assert_eq!(
            manifest.parsed.project.platforms.value,
            vec![Platform::Linux64, Platform::Win64]
                .into_iter()
                .collect::<IndexSet<_>>()
        );

        manifest
            .add_platforms([Platform::OsxArm64].iter(), &FeatureName::Default)
            .unwrap();

        assert_eq!(
            manifest.parsed.project.platforms.value,
            vec![Platform::Linux64, Platform::Win64, Platform::OsxArm64]
                .into_iter()
                .collect::<IndexSet<_>>()
        );

        manifest
            .add_platforms(
                [Platform::LinuxAarch64, Platform::Osx64].iter(),
                &FeatureName::Named("test".to_string()),
            )
            .unwrap();

        assert_eq!(
            manifest
                .feature(&FeatureName::Named("test".to_string()))
                .unwrap()
                .platforms
                .clone()
                .unwrap()
                .value,
            vec![Platform::LinuxAarch64, Platform::Osx64]
                .into_iter()
                .collect::<IndexSet<_>>()
        );

        manifest
            .add_platforms(
                [Platform::LinuxAarch64, Platform::Win64].iter(),
                &FeatureName::Named("test".to_string()),
            )
            .unwrap();

        assert_eq!(
            manifest
                .feature(&FeatureName::Named("test".to_string()))
                .unwrap()
                .platforms
                .clone()
                .unwrap()
                .value,
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

        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        assert_eq!(
            manifest.parsed.project.platforms.value,
            vec![Platform::Linux64, Platform::Win64]
                .into_iter()
                .collect::<IndexSet<_>>()
        );

        manifest
            .remove_platforms(vec![Platform::Linux64], &FeatureName::Default)
            .unwrap();

        assert_eq!(
            manifest.parsed.project.platforms.value,
            vec![Platform::Win64].into_iter().collect::<IndexSet<_>>()
        );

        assert_eq!(
            manifest
                .feature(&FeatureName::Named("test".to_string()))
                .unwrap()
                .platforms
                .clone()
                .unwrap()
                .value,
            vec![Platform::Linux64, Platform::Win64, Platform::Osx64]
                .into_iter()
                .collect::<IndexSet<_>>()
        );

        manifest
            .remove_platforms(
                vec![Platform::Linux64, Platform::Osx64],
                &FeatureName::Named("test".to_string()),
            )
            .unwrap();

        assert_eq!(
            manifest
                .feature(&FeatureName::Named("test".to_string()))
                .unwrap()
                .platforms
                .clone()
                .unwrap()
                .value,
            vec![Platform::Win64].into_iter().collect::<IndexSet<_>>()
        );

        // Test removing non-existing platforms
        assert!(manifest
            .remove_platforms(
                vec![Platform::Linux64, Platform::Osx64],
                &FeatureName::Named("test".to_string()),
            )
            .is_err());
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

        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        assert_eq!(manifest.parsed.project.channels, IndexSet::new());

        let conda_forge = PrioritizedChannel::from(
            NamedChannelOrUrl::Name(String::from("conda-forge")),
        );
        manifest
            .add_channels([conda_forge.clone()], &FeatureName::Default)
            .unwrap();

        let cuda_feature = FeatureName::Named("cuda".to_string());
        let nvidia = PrioritizedChannel::from(
            NamedChannelOrUrl::Name(String::from("nvidia")),
        );
        manifest
            .add_channels([nvidia.clone()], &cuda_feature)
            .unwrap();

        let test_feature = FeatureName::Named("test".to_string());
        manifest
            .add_channels(
                [
                    PrioritizedChannel::from(
                        NamedChannelOrUrl::Name(String::from("test")),
                    ),
                    PrioritizedChannel::from(
                        NamedChannelOrUrl::Name(String::from("test2")),
                    ),
                ],
                &test_feature,
            )
            .unwrap();

        assert_eq!(
            manifest.parsed.project.channels,
            vec![PrioritizedChannel {
                channel: NamedChannelOrUrl::Name(String::from("conda-forge")),
                priority: None,
            }]
            .into_iter()
            .collect::<IndexSet<_>>()
        );

        // Try to add again, should not add more channels
        manifest
            .add_channels([conda_forge.clone()], &FeatureName::Default)
            .unwrap();

        assert_eq!(
            manifest.parsed.project.channels,
            vec![PrioritizedChannel {
                channel: NamedChannelOrUrl::Name(String::from("conda-forge")),
                priority: None,
            }]
            .into_iter()
            .collect::<IndexSet<_>>()
        );

        assert_eq!(
            manifest
                .parsed
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
            .add_channels([nvidia.clone()], &cuda_feature)
            .unwrap();
        assert_eq!(
            manifest
                .parsed
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
                .parsed
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
            .add_channels([custom_channel.clone()], &FeatureName::Default)
            .unwrap();
        assert!(manifest
            .parsed
            .project
            .channels
            .iter()
            .any(|c| c.channel == custom_channel.channel));

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

        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        assert_eq!(
            manifest.parsed.project.channels,
            vec![PrioritizedChannel::from(
                NamedChannelOrUrl::Name(String::from("conda-forge"))
            )]
            .into_iter()
            .collect::<IndexSet<_>>()
        );

        manifest
            .remove_channels(
                [PrioritizedChannel {
                    channel: NamedChannelOrUrl::Name(String::from("conda-forge")),
                    priority: None,
                }],
                &FeatureName::Default,
            )
            .unwrap();

        assert_eq!(manifest.parsed.project.channels, IndexSet::new());

        manifest
            .remove_channels(
                [PrioritizedChannel {
                    channel: NamedChannelOrUrl::Name(String::from("test_channel")),
                    priority: None,
                }],
                &FeatureName::Named("test".to_string()),
            )
            .unwrap();

        let feature_channels = manifest
            .feature(&FeatureName::Named("test".to_string()))
            .unwrap()
            .channels
            .clone()
            .unwrap();
        assert_eq!(feature_channels, IndexSet::new());

        // Test failing to remove a channel that does not exist
        assert!(manifest
            .remove_channels(
                [PrioritizedChannel {
                    channel: NamedChannelOrUrl::Name(String::from("conda-forge")),
                    priority: None,
                }],
                &FeatureName::Default,
            )
            .is_err());
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
        let manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();
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
                .map(|idx| manifest.parsed.solve_groups[idx].name.as_str()),
            Some("test")
        );

        let test2_env = manifest
            .environment(&EnvironmentName::Named("test2".to_string()))
            .unwrap();
        assert_eq!(test2_env.features, vec!["py39"]);
        assert_eq!(
            test2_env
                .solve_group
                .map(|idx| manifest.parsed.solve_groups[idx].name.as_str()),
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
        let manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        let cuda_feature = manifest
            .parsed
            .features
            .get(&FeatureName::Named("cuda".to_string()))
            .unwrap();
        assert_eq!(cuda_feature.name, FeatureName::Named("cuda".to_string()));
        assert_eq!(
            cuda_feature
                .targets
                .default()
                .dependencies
                .get(&SpecType::Run)
                .unwrap()
                .get(&PackageName::from_str("cuda").unwrap())
                .unwrap()
                .to_string(),
            "==x.y.z"
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
                .to_string(),
            "==12.0"
        );
        assert_eq!(
            cuda_feature
                .targets
                .default()
                .pypi_dependencies
                .as_ref()
                .unwrap()
                .get(&PyPiPackageName::from_str("torch").expect("torch should be a valid name"))
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
                .to_string(),
            "*"
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
                .to_string(),
            "==x.y.z"
        );
        assert_eq!(
            cuda_feature
                .targets
                .default()
                .tasks
                .get(&"warmup".into())
                .unwrap()
                .as_single_command()
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
        let manifest = Manifest::from_str(
            Path::new("pixi.toml"),
            format!("{PROJECT_BOILERPLATE}\n{file_contents}").as_str(),
        )
        .unwrap();
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

        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        manifest
            .add_task(
                "default".into(),
                Task::Plain("echo default".to_string()),
                None,
                &FeatureName::Default,
            )
            .unwrap();
        manifest
            .add_task(
                "target_linux".into(),
                Task::Plain("echo target_linux".to_string()),
                Some(Platform::Linux64),
                &FeatureName::Default,
            )
            .unwrap();
        manifest
            .add_task(
                "feature_test".into(),
                Task::Plain("echo feature_test".to_string()),
                None,
                &FeatureName::Named("test".to_string()),
            )
            .unwrap();
        manifest
            .add_task(
                "feature_test_target_linux".into(),
                Task::Plain("echo feature_test_target_linux".to_string()),
                Some(Platform::Linux64),
                &FeatureName::Named("test".to_string()),
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
        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();
        manifest
            .add_dependency(
                &MatchSpec::from_str("baz >=1.2.3", Strict).unwrap(),
                SpecType::Run,
                &[],
                &FeatureName::Default,
                DependencyOverwriteBehavior::Overwrite,
                &default_channel_config()
            )
            .unwrap();
        assert_eq!(
            manifest
                .default_feature()
                .targets
                .default()
                .dependencies
                .get(&SpecType::Run)
                .unwrap()
                .get(&PackageName::from_str("baz").unwrap())
                .unwrap()
                .to_string(),
            ">=1.2.3".to_string()
        );
        manifest
            .add_dependency(
                &MatchSpec::from_str(" bal >=2.3", Strict).unwrap(),
                SpecType::Run,
                &[],
                &FeatureName::Named("test".to_string()),
                DependencyOverwriteBehavior::Overwrite,
                &default_channel_config()
            )
            .unwrap();

        assert_eq!(
            manifest
                .feature(&FeatureName::Named("test".to_string()))
                .unwrap()
                .targets
                .default()
                .dependencies
                .get(&SpecType::Run)
                .unwrap()
                .get(&PackageName::from_str("bal").unwrap())
                .unwrap()
                .to_string(),
            ">=2.3".to_string()
        );

        manifest
            .add_dependency(
                &MatchSpec::from_str(" boef >=2.3", Strict).unwrap(),
                SpecType::Run,
                &[Platform::Linux64],
                &FeatureName::Named("extra".to_string()),
                DependencyOverwriteBehavior::Overwrite,
                &default_channel_config()
            )
            .unwrap();

        assert_eq!(
            manifest
                .feature(&FeatureName::Named("extra".to_string()))
                .unwrap()
                .targets
                .for_target(&TargetSelector::Platform(Platform::Linux64))
                .unwrap()
                .dependencies
                .get(&SpecType::Run)
                .unwrap()
                .get(&PackageName::from_str("boef").unwrap())
                .unwrap()
                .to_string(),
            ">=2.3".to_string()
        );

        manifest
            .add_dependency(
                &MatchSpec::from_str(" cmake >=2.3", ParseStrictness::Strict).unwrap(),
                SpecType::Build,
                &[Platform::Linux64],
                &FeatureName::Named("build".to_string()),
                DependencyOverwriteBehavior::Overwrite,
                &default_channel_config()
            )
            .unwrap();

        assert_eq!(
            manifest
                .feature(&FeatureName::Named("build".to_string()))
                .unwrap()
                .targets
                .for_target(&TargetSelector::Platform(Platform::Linux64))
                .unwrap()
                .dependencies
                .get(&SpecType::Build)
                .unwrap()
                .get(&PackageName::from_str("cmake").unwrap())
                .unwrap()
                .to_string(),
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
        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), contents).unwrap();
        manifest
            .add_environment(String::from("test"), Some(Vec::new()), None, false)
            .unwrap();
        assert!(manifest.environment("test").is_some());
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
        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), contents).unwrap();
        manifest
            .add_environment(
                String::from("test"),
                Some(vec![String::from("foobar")]),
                None,
                false,
            )
            .unwrap();
        assert!(manifest.environment("test").is_some());
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
        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), contents).unwrap();
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
        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), contents).unwrap();
        assert!(manifest.remove_environment("foo").unwrap());
        assert!(!manifest.remove_environment("default").unwrap());
    }

    #[test]
    pub fn test_channel_priority_manifest() {
        let manifest = Manifest::from_str(
            Path::new("pixi.toml"),
            r#"
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
        "#,
        )
        .unwrap();

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

        let manifest = Manifest::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foo"
        platforms = []
        channels = []
        channel-priority = "disabled"
        "#,
        )
        .unwrap();

        assert_eq!(
            manifest.default_feature().channel_priority.unwrap(),
            ChannelPriority::Disabled
        );
    }
}
