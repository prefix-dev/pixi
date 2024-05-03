mod activation;
pub(crate) mod channel;
mod document;
mod environment;
mod error;
mod feature;
mod metadata;
pub mod pypi_options;
pub mod pyproject;
pub mod python;
mod system_requirements;
mod target;
mod validation;

use crate::config::Config;
use crate::project::manifest::channel::PrioritizedChannel;
use crate::project::manifest::environment::TomlEnvironmentMapOrSeq;
use crate::project::manifest::pypi_options::PypiOptions;
use crate::project::manifest::python::PyPiPackageName;
use crate::pypi_mapping::{ChannelName, MappingLocation, MappingSource};
use crate::task::TaskName;
use crate::{consts, project::SpecType, task::Task, utils::spanned::PixiSpanned};
pub use activation::Activation;
use document::ManifestSource;
pub use environment::{Environment, EnvironmentName};
pub use feature::{Feature, FeatureName};
use indexmap::{Equivalent, IndexMap, IndexSet};
use itertools::Itertools;
pub use metadata::ProjectMetadata;
use miette::{miette, Diagnostic, IntoDiagnostic, NamedSource, WrapErr};
use once_cell::sync::OnceCell;
use pyproject::PyProjectManifest;
pub use python::PyPiRequirement;
use rattler_conda_types::Channel;
use rattler_conda_types::{
    MatchSpec, NamelessMatchSpec, PackageName,
    ParseStrictness::{Lenient, Strict},
    Platform, Version,
};
use serde::de::{DeserializeSeed, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_with::serde_as;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt::{self, Display};
use std::hash::Hash;
use std::marker::PhantomData;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
};
pub use system_requirements::{LibCSystemRequirement, SystemRequirements};
pub use target::{Target, TargetSelector, Targets};
use thiserror::Error;
use toml_edit::DocumentMut;

use self::error::TomlError;
use url::ParseError;
use url::Url;

/// Errors that can occur when getting a feature.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum GetFeatureError {
    #[error("feature `{0}` does not exist")]
    FeatureDoesNotExist(FeatureName),
}

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
/// This struct is responsible for reading, parsing, editing, and saving the manifest.
/// It encapsulates all logic related to the manifest's TOML format and structure.
/// The manifest data is represented as a [`ProjectManifest`] struct for easy manipulation.
/// Owned by the [`crate::project::Project`] struct, which governs its usage.
///
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The path to the manifest file
    pub path: PathBuf,

    /// The raw contents of the manifest file
    pub contents: String,

    /// Editable toml document
    pub document: ManifestSource,

    /// The parsed manifest
    pub parsed: ProjectManifest,
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
            ManifestKind::Pixi => (ProjectManifest::from_toml_str(&contents), "pixi.toml"),
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

    /// Returns a hashmap of the tasks that should run only the given platform. If the platform is
    /// `None`, only the default targets tasks are returned.
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
                miette::bail!("task {} already exists", name.fancy_display());
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
            .ok_or_else(|| miette::miette!("task {} does not exist", name.fancy_display()))?;

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
        let to_remove: IndexSet<_> = platforms.into_iter().collect();
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
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Determine the name of the package to add
        let (Some(name), spec) = spec.clone().into_nameless() else {
            miette::bail!("pixi does not support wildcard dependencies")
        };

        // Add the dependency to the manifest
        self.get_or_insert_target_mut(platform, Some(feature_name))
            .try_add_dependency(&name, &spec, spec_type)?;
        // and to the TOML document
        self.document
            .add_dependency(&name, &spec, spec_type, platform, feature_name)?;
        Ok(())
    }

    /// Add a pypi requirement to the manifest
    pub fn add_pypi_dependency(
        &mut self,
        requirement: &pep508_rs::Requirement,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // Add the pypi dependency to the manifest
        self.get_or_insert_target_mut(platform, Some(feature_name))
            .try_add_pypi_dependency(requirement)?;
        // and to the TOML document
        self.document
            .add_pypi_dependency(requirement, platform, feature_name)?;
        Ok(())
    }

    /// Removes a dependency based on `SpecType`.
    pub fn remove_dependency(
        &mut self,
        dep: &PackageName,
        spec_type: SpecType,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> miette::Result<(PackageName, NamelessMatchSpec)> {
        // Remove the dependency from the manifest
        let res = self
            .target_mut(platform, feature_name)
            .remove_dependency(dep, spec_type);
        // Remove the dependency from the TOML document
        self.document
            .remove_dependency(dep, spec_type, platform, feature_name)?;
        res.into_diagnostic()
    }

    /// Removes a pypi dependency.
    pub fn remove_pypi_dependency(
        &mut self,
        dep: &PyPiPackageName,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> miette::Result<(PyPiPackageName, PyPiRequirement)> {
        // Remove the dependency from the manifest
        let res = self
            .target_mut(platform, feature_name)
            .remove_pypi_dependency(dep);
        // Remove the dependency from the TOML document
        self.document
            .remove_pypi_dependency(dep, platform, feature_name)?;
        res.into_diagnostic()
    }

    /// Returns true if any of the features has pypi dependencies defined.
    ///
    /// This also returns true if the `pypi-dependencies` key is defined but empty.
    pub fn has_pypi_dependencies(&self) -> bool {
        self.parsed
            .features
            .values()
            .flat_map(|f| f.targets.targets())
            .any(|f| f.pypi_dependencies.is_some())
    }

    /// Returns what pypi mapping configuration we should use.
    /// It can be a custom one  in following format : conda_name: pypi_name
    /// Or we can use our self-hosted
    pub fn pypi_name_mapping_source(&self) -> miette::Result<&'static MappingSource> {
        static MAPPING_SOURCE: OnceCell<MappingSource> = OnceCell::new();

        MAPPING_SOURCE.get_or_try_init(||
            match self.parsed.project.conda_pypi_map.clone() {
                Some(url) => {
                    let config = Config::load_global();

                    // transform user defined channels into rattler::Channel
                    let channels = url
                        .keys()
                        .map(|channel_str| {
                            Channel::from_str(channel_str, config.channel_config())
                                .map(|channel| (channel_str, channel.canonical_name()))
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .into_diagnostic()?;

                    let project_channels = self.parsed.project.channels
                    .iter()
                    .map(|pc| pc.channel.canonical_name())
                    .collect::<HashSet<_>>();


                    // Throw a warning for each missing channel from project table
                    channels
                    .iter()
                    .for_each(|(_, channel_canonical_name)| {
                        if !project_channels.contains(channel_canonical_name){
                            tracing::warn!("Defined custom mapping channel {} is missing from project channels", channel_canonical_name);
                        }
                    });

                    let mapping = channels
                            .iter()
                            .map(|(channel_str, channel)| {

                                let mapping_location = url.get(*channel_str).unwrap();

                                let url_or_path = match Url::parse(mapping_location) {
                                    Ok(url) => MappingLocation::Url(url),
                                    Err(err) => {
                                        if let ParseError::RelativeUrlWithoutBase = err {
                                            MappingLocation::Path(PathBuf::from(mapping_location))
                                        } else {
                                            miette::bail!("Could not convert {mapping_location} to neither URL or Path")
                                        }
                                    }
                                };

                                Ok((channel.clone(), url_or_path))
                            })
                            .collect::<miette::Result<HashMap<ChannelName, MappingLocation>>>()?;

                    Ok(MappingSource::Custom { mapping })
                },
                None => Ok(MappingSource::Prefix),
            }
        )
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
            match feature_name {
                FeatureName::Default => channels.push(channel.to_name_or_url()),
                FeatureName::Named(_) => channels.push(channel.channel.name().to_string()),
            }
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
        let to_remove: IndexSet<_> = channels.into_iter().collect();
        let retained: IndexSet<_> = current.difference(&to_remove).cloned().collect();

        // Remove channels from the manifest
        current.retain(|c| retained.contains(c));

        // And from the TOML document
        let retained = retained.iter().map(|c| c.channel.name()).collect_vec();
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
    /// This is the feature that is added implicitly by the tables at the root of the project
    /// manifest.
    pub fn default_feature(&self) -> &Feature {
        self.parsed.default_feature()
    }

    /// Returns a mutable reference to the default feature.
    fn default_feature_mut(&mut self) -> &mut Feature {
        self.parsed.default_feature_mut()
    }

    /// Returns the mutable feature with the given name or `None` if it does not exist.
    pub fn feature_mut<Q: ?Sized>(&mut self, name: &Q) -> miette::Result<&mut Feature>
    where
        Q: Hash + Equivalent<FeatureName> + Display,
    {
        self.parsed
            .features
            .get_mut(name)
            .ok_or_else(|| miette!("Feature `{name}` does not exist"))
    }

    /// Returns the mutable feature with the given name or `None` if it does not exist.
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
    /// This is the environment that is added implicitly as the environment with only the default
    /// feature. The default environment can be overwritten by a environment named `default`.
    pub fn default_environment(&self) -> &Environment {
        self.parsed.default_environment()
    }

    /// Returns the environment with the given name or `None` if it does not exist.
    pub fn environment<Q: ?Sized>(&self, name: &Q) -> Option<&Environment>
    where
        Q: Hash + Equivalent<EnvironmentName>,
    {
        self.parsed.environments.find(name)
    }

    /// Returns the solve group with the given name or `None` if it does not exist.
    pub fn solve_group<Q: ?Sized>(&self, name: &Q) -> Option<&SolveGroup>
    where
        Q: Hash + Equivalent<String>,
    {
        self.parsed.solve_groups.find(name)
    }
}

/// The environments in the project.
#[derive(Debug, Clone, Default)]
pub struct Environments {
    /// A list of all environments, in the order they are defined in the manifest.
    pub(super) environments: Vec<Environment>,

    /// A map of all environments, indexed by their name.
    pub(super) by_name: IndexMap<EnvironmentName, usize>,
}

impl Environments {
    /// Returns the environment with the given name or `None` if it does not exist.
    pub fn find<Q: ?Sized>(&self, name: &Q) -> Option<&Environment>
    where
        Q: Hash + Equivalent<EnvironmentName>,
    {
        let index = self.by_name.get(name)?;
        Some(&self.environments[*index])
    }

    /// Returns an iterator over all the environments in the project.
    pub fn iter(&self) -> impl Iterator<Item = &Environment> + '_ {
        self.environments.iter()
    }
}

/// A solve group is a group of environments that are solved together.
#[derive(Debug, Clone)]
pub struct SolveGroup {
    pub name: String,
    pub environments: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct SolveGroups {
    pub(super) solve_groups: Vec<SolveGroup>,
    pub(super) by_name: IndexMap<String, usize>,
}

impl SolveGroups {
    /// Returns the solve group with the given name or `None` if it does not exist.
    pub fn find<Q: ?Sized>(&self, name: &Q) -> Option<&SolveGroup>
    where
        Q: Hash + Equivalent<String>,
    {
        let index = self.by_name.get(name)?;
        Some(&self.solve_groups[*index])
    }

    /// Returns an iterator over all the solve groups in the project.
    pub fn iter(&self) -> impl Iterator<Item = &SolveGroup> + '_ {
        self.solve_groups.iter()
    }

    /// Adds an environment (by index) to a solve-group.
    /// If the solve-group does not exist, it is created
    ///
    /// Returns the index of the solve-group
    fn add(&mut self, name: &str, environment_idx: usize) -> usize {
        match self.by_name.get(name) {
            Some(idx) => {
                // The solve-group exists, add the environment index to it
                self.solve_groups[*idx].environments.push(environment_idx);
                *idx
            }
            None => {
                // The solve-group does not exist, create it
                // and initialise it with the environment index
                let idx = self.solve_groups.len();
                self.solve_groups.push(SolveGroup {
                    name: name.to_string(),
                    environments: vec![environment_idx],
                });
                self.by_name.insert(name.to_string(), idx);
                idx
            }
        }
    }
}

/// Describes the contents of a project manifest.
#[derive(Debug, Clone)]
pub struct ProjectManifest {
    /// Information about the project
    pub project: ProjectMetadata,

    /// All the features defined in the project.
    pub features: IndexMap<FeatureName, Feature>,

    /// All the environments defined in the project.
    pub environments: Environments,

    /// The solve groups that are part of the project.
    pub solve_groups: SolveGroups,
}

impl ProjectManifest {
    /// Parses a toml string into a project manifest.
    pub fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        let manifest: ProjectManifest = toml_edit::de::from_str(source).map_err(TomlError::from)?;

        // Make sure project.name is defined
        if manifest.project.name.is_none() {
            let span = source.parse::<DocumentMut>().map_err(TomlError::from)?["project"].span();
            return Err(TomlError::NoProjectName(span));
        }

        Ok(manifest)
    }

    /// Returns the default feature.
    ///
    /// This is the feature that is added implicitly by the tables at the root of the project
    /// manifest.
    pub fn default_feature(&self) -> &Feature {
        self.features
            .get(&FeatureName::Default)
            .expect("default feature should always exist")
    }

    /// Returns a mutable reference to the default feature.
    fn default_feature_mut(&mut self) -> &mut Feature {
        self.features
            .get_mut(&FeatureName::Default)
            .expect("default feature should always exist")
    }

    /// Returns the default environment
    ///
    /// This is the environment that is added implicitly as the environment with only the default
    /// feature. The default environment can be overwritten by a environment named `default`.
    pub fn default_environment(&self) -> &Environment {
        let envs = &self.environments;
        envs.find(&EnvironmentName::Named(String::from(
            consts::DEFAULT_ENVIRONMENT_NAME,
        )))
        .or_else(|| envs.find(&EnvironmentName::Default))
        .expect("default environment should always exist")
    }

    /// Returns the environment with the given name or `None` if it does not exist.
    pub fn environment<Q: ?Sized>(&self, name: &Q) -> Option<&Environment>
    where
        Q: Hash + Equivalent<EnvironmentName>,
    {
        self.environments.find(name)
    }
}

struct PackageMap<'a>(&'a IndexMap<PackageName, NamelessMatchSpec>);

impl<'de, 'a> DeserializeSeed<'de> for PackageMap<'a> {
    type Value = PackageName;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        let package_name = PackageName::deserialize(deserializer)?;
        match self.0.get_key_value(&package_name) {
            Some((package_name, _)) => {
                 Err(serde::de::Error::custom(
                    format!(
                    "duplicate dependency: {} (please avoid using capitalized names for the dependencies)", package_name.as_source())
                ))
            }
            None => Ok(package_name),
        }
    }
}

struct NamelessMatchSpecWrapper {}

impl<'de, 'a> DeserializeSeed<'de> for &'a NamelessMatchSpecWrapper {
    type Value = NamelessMatchSpec;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .string(|str| {
                match NamelessMatchSpec::from_str(str, Strict) {
                    Ok(spec) => Ok(spec),
                    Err(_) => {
                        let spec = NamelessMatchSpec::from_str(str, Lenient).map_err(serde::de::Error::custom)?;
                        tracing::warn!("Parsed '{str}' as '{spec}', in a future version this will become an error.", spec=&spec);
                        Ok(spec)
                    }
                }
            })
            .map(|map| {
                NamelessMatchSpec::deserialize(serde::de::value::MapAccessDeserializer::new(map))
            })
            .expecting("either a map or a string")
            .deserialize(deserializer)
    }
}

pub(crate) fn deserialize_package_map<'de, D>(
    deserializer: D,
) -> Result<IndexMap<PackageName, NamelessMatchSpec>, D::Error>
where
    D: Deserializer<'de>,
{
    struct PackageMapVisitor(PhantomData<()>);

    impl<'de> Visitor<'de> for PackageMapVisitor {
        type Value = IndexMap<PackageName, NamelessMatchSpec>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "a map")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut result = IndexMap::new();
            let match_spec = NamelessMatchSpecWrapper {};
            while let Some((package_name, match_spec)) = map
                .next_entry_seed::<PackageMap, &NamelessMatchSpecWrapper>(
                    PackageMap(&result),
                    &match_spec,
                )?
            {
                result.insert(package_name, match_spec);
            }

            Ok(result)
        }
    }
    let visitor = PackageMapVisitor(PhantomData);
    deserializer.deserialize_seq(visitor)
}

pub(crate) fn deserialize_opt_package_map<'de, D>(
    deserializer: D,
) -> Result<Option<IndexMap<PackageName, NamelessMatchSpec>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(deserialize_package_map(deserializer)?))
}

impl<'de> Deserialize<'de> for ProjectManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[serde_as]
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "kebab-case")]
        pub struct TomlProjectManifest {
            project: ProjectMetadata,
            #[serde(default)]
            system_requirements: SystemRequirements,
            #[serde(default)]
            target: IndexMap<PixiSpanned<TargetSelector>, Target>,

            // HACK: If we use `flatten`, unknown keys will point to the wrong location in the file.
            //  When https://github.com/toml-rs/toml/issues/589 is fixed we should use that
            //
            // Instead we currently copy the keys from the Target deserialize implementation which
            // is really ugly.
            //
            // #[serde(flatten)]
            // default_target: Target,
            #[serde(default, deserialize_with = "deserialize_package_map")]
            dependencies: IndexMap<PackageName, NamelessMatchSpec>,

            #[serde(default, deserialize_with = "deserialize_opt_package_map")]
            host_dependencies: Option<IndexMap<PackageName, NamelessMatchSpec>>,

            #[serde(default, deserialize_with = "deserialize_opt_package_map")]
            build_dependencies: Option<IndexMap<PackageName, NamelessMatchSpec>>,

            #[serde(default)]
            pypi_dependencies: Option<IndexMap<PyPiPackageName, PyPiRequirement>>,

            /// Additional information to activate an environment.
            #[serde(default)]
            activation: Option<Activation>,

            /// Target specific tasks to run in the environment
            #[serde(default)]
            tasks: HashMap<TaskName, Task>,

            /// The features defined in the project.
            #[serde(default)]
            feature: IndexMap<FeatureName, Feature>,

            /// The environments the project can create.
            #[serde(default)]
            environments: IndexMap<EnvironmentName, TomlEnvironmentMapOrSeq>,

            #[serde(default)]
            pypi_options: Option<PypiOptions>,

            /// The tool configuration which is unused by pixi
            #[serde(rename = "tool")]
            _tool: Option<serde_json::Value>,

            /// The URI for the manifest schema which is unused by pixi
            #[allow(dead_code)]
            #[serde(rename = "$schema")]
            schema: Option<String>,
        }

        let toml_manifest = TomlProjectManifest::deserialize(deserializer)?;
        let mut dependencies = HashMap::from_iter([(SpecType::Run, toml_manifest.dependencies)]);
        if let Some(host_deps) = toml_manifest.host_dependencies {
            dependencies.insert(SpecType::Host, host_deps);
        }
        if let Some(build_deps) = toml_manifest.build_dependencies {
            dependencies.insert(SpecType::Build, build_deps);
        }

        let default_target = Target {
            dependencies,
            pypi_dependencies: toml_manifest.pypi_dependencies,
            activation: toml_manifest.activation,
            tasks: toml_manifest.tasks,
        };

        // Construct a default feature
        let default_feature = Feature {
            name: FeatureName::Default,

            // The default feature does not overwrite the platforms or channels from the project
            // metadata.
            platforms: None,
            channels: None,

            system_requirements: toml_manifest.system_requirements,
            pypi_options: toml_manifest.pypi_options,

            // Combine the default target with all user specified targets
            targets: Targets::from_default_and_user_defined(default_target, toml_manifest.target),
        };

        // Construct the features including the default feature
        let features: IndexMap<FeatureName, Feature> =
            IndexMap::from_iter([(FeatureName::Default, default_feature)]);
        let named_features = toml_manifest
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
        if !toml_manifest
            .environments
            .contains_key(&EnvironmentName::Default)
        {
            environments.environments.push(Environment::default());
            environments.by_name.insert(EnvironmentName::Default, 0);
        }

        // Add all named environments
        for (name, env) in toml_manifest.environments {
            // Decompose the TOML
            let (features, features_source_loc, solve_group, no_default_feature) = match env {
                TomlEnvironmentMapOrSeq::Map(env) => (
                    env.features.value,
                    env.features.span,
                    env.solve_group,
                    env.no_default_feature,
                ),
                TomlEnvironmentMapOrSeq::Seq(features) => (features, None, None, false),
            };

            let environment_idx = environments.environments.len();
            environments.by_name.insert(name.clone(), environment_idx);
            environments.environments.push(Environment {
                name,
                features,
                features_source_loc,
                solve_group: solve_group.map(|sg| solve_groups.add(&sg, environment_idx)),
                no_default_feature,
            });
        }

        Ok(Self {
            project: toml_manifest.project,
            features,
            environments,
            solve_groups,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{project::manifest::channel::PrioritizedChannel, util::default_channel_config};
    use insta::{assert_snapshot, assert_yaml_snapshot};
    use rattler_conda_types::{Channel, ChannelConfig, ParseStrictness};
    use rstest::*;
    use std::str::FromStr;
    use tempfile::tempdir;

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = ["linux-64", "win-64", "osx-64"]
        "#;

    fn channel_config() -> ChannelConfig {
        default_channel_config()
    }

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
    fn test_target_specific() {
        let contents = format!(
            r#"
        {PROJECT_BOILERPLATE}

        [target.win-64.dependencies]
        foo = "3.4.5"

        [target.osx-64.dependencies]
        foo = "1.2.3"
        "#
        );

        let manifest = ProjectManifest::from_toml_str(&contents).unwrap();
        let targets = &manifest.default_feature().targets;
        assert_eq!(
            targets.user_defined_selectors().cloned().collect_vec(),
            vec![
                TargetSelector::Platform(Platform::Win64),
                TargetSelector::Platform(Platform::Osx64)
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
                .to_string(),
            "==3.4.5"
        );
        assert_eq!(
            osx64_target
                .run_dependencies()
                .unwrap()
                .get("foo")
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

        let manifest = ProjectManifest::from_toml_str(&contents).unwrap();
        let deps = manifest
            .default_feature()
            .targets
            .default()
            .run_dependencies()
            .unwrap();
        let test_map_spec = deps.get("test_map").unwrap();

        assert_eq!(test_map_spec.to_string(), ">=1.2.3 py34_0");
        assert_eq!(
            test_map_spec
                .channel
                .as_deref()
                .map(Channel::canonical_name),
            Some(String::from("https://conda.anaconda.org/conda-forge/"))
        );

        assert_eq!(deps.get("test_build").unwrap().to_string(), "* bla");

        let test_channel = deps.get("test_channel").unwrap();
        assert_eq!(test_channel.to_string(), "*");
        assert_eq!(
            test_channel.channel.as_deref().map(Channel::canonical_name),
            Some(String::from("https://conda.anaconda.org/conda-forge/"))
        );

        let test_version = deps.get("test_version").unwrap();
        assert_eq!(test_version.to_string(), ">=1.2.3");

        let test_version_channel = deps.get("test_version_channel").unwrap();
        assert_eq!(test_version_channel.to_string(), ">=1.2.3");
        assert_eq!(
            test_version_channel
                .channel
                .as_deref()
                .map(Channel::canonical_name),
            Some(String::from("https://conda.anaconda.org/conda-forge/"))
        );

        let test_version_build = deps.get("test_version_build").unwrap();
        assert_eq!(test_version_build.to_string(), ">=1.2.3 py34_0");
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

        let manifest = ProjectManifest::from_toml_str(&contents).unwrap();
        let default_target = manifest.default_feature().targets.default();
        let run_dependencies = default_target.run_dependencies().unwrap();
        let build_dependencies = default_target.build_dependencies().unwrap();
        let host_dependencies = default_target.host_dependencies().unwrap();

        assert_eq!(
            run_dependencies.get("my-game").unwrap().to_string(),
            "==1.0.0"
        );
        assert_eq!(build_dependencies.get("cmake").unwrap().to_string(), "*");
        assert_eq!(host_dependencies.get("sdl2").unwrap().to_string(), "*");
    }

    #[test]
    fn test_invalid_target_specific() {
        let examples = [r#"[target.foobar.dependencies]
            invalid_platform = "henk""#];

        assert_snapshot!(examples
            .into_iter()
            .map(|example| ProjectManifest::from_toml_str(&format!(
                "{PROJECT_BOILERPLATE}\n{example}"
            ))
            .unwrap_err()
            .to_string())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[test]
    fn test_invalid_key() {
        let examples = [
            format!("{PROJECT_BOILERPLATE}\n[foobar]"),
            format!("{PROJECT_BOILERPLATE}\n[target.win-64.hostdependencies]"),
        ];
        assert_snapshot!(examples
            .into_iter()
            .map(|example| ProjectManifest::from_toml_str(&example)
                .unwrap_err()
                .to_string())
            .collect::<Vec<_>>()
            .join("\n"))
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
                String::from("test")
            ])
        );
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

        let manifest = ProjectManifest::from_toml_str(&contents).unwrap();

        assert_snapshot!(manifest
            .default_feature()
            .targets
            .iter()
            .flat_map(|(target, selector)| {
                let selector_name =
                    selector.map_or_else(|| String::from("default"), ToString::to_string);
                target.tasks.iter().filter_map(move |(name, task)| {
                    Some(format!(
                        "{}/{} = {}",
                        &selector_name,
                        name.as_str(),
                        task.as_single_command()?
                    ))
                })
            })
            .join("\n"));
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

        assert_snapshot!(toml_edit::de::from_str::<ProjectManifest>(&contents)
            .expect("parsing should succeed!")
            .default_feature()
            .targets
            .default()
            .pypi_dependencies
            .clone()
            .into_iter()
            .flat_map(|d| d.into_iter())
            .map(|(name, spec)| format!("{} = {}", name.as_source(), toml_edit::Value::from(spec)))
            .join("\n"));
    }

    #[test]
    fn test_pypi_options_default_feature() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [pypi-options]
            index-url = "https://pypi.org/simple"
            extra-index-urls = ["https://pypi.org/simple2"]
            [[pypi-options.find-links]]
            path = "../foo"
            [[pypi-options.find-links]]
            url = "https://example.com/bar"
            "#
        );

        assert_yaml_snapshot!(toml_edit::de::from_str::<ProjectManifest>(&contents)
            .expect("parsing should succeed!")
            .default_feature()
            .pypi_options
            .clone()
            .unwrap());
    }

    fn test_remove(
        file_contents: &str,
        name: &str,
        kind: SpecType,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) {
        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        // Initially the dependency should exist
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

        // Remove the dependency from the manifest
        manifest
            .remove_dependency(
                &PackageName::new_unchecked(name),
                kind,
                platform,
                feature_name,
            )
            .unwrap();

        // The dependency should no longer exist
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

        // Write the toml to string and verify the content
        assert_snapshot!(
            format!("test_remove_{}", name),
            manifest.document.to_string()
        );
    }

    fn test_remove_pypi(
        file_contents: &str,
        name: &str,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) {
        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        let package_name = PyPiPackageName::from_str(name).unwrap();

        // Initially the dependency should exist
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

        // Remove the dependency from the manifest
        manifest
            .remove_pypi_dependency(&package_name, platform, feature_name)
            .unwrap();

        // The dependency should no longer exist
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

        // Write the toml to string and verify the content
        assert_snapshot!(
            format!("test_remove_pypi_{}", name),
            manifest.document.to_string()
        );
    }

    #[rstest]
    #[case::xpackage("xpackage", Some(Platform::Linux64), FeatureName::Default)]
    #[case::jax("jax", Some(Platform::Win64), FeatureName::Default)]
    #[case::requests("requests", None, FeatureName::Default)]
    #[case::feature_dep("feature_dep", None, FeatureName::Named("test".to_string()))]
    #[case::feature_target_dep("feature_target_dep", Some(Platform::Linux64), FeatureName::Named("test".to_string()))]
    fn test_remove_pypi_dependencies(
        #[case] package_name: &str,
        #[case] platform: Option<Platform>,
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
        test_remove_pypi(pixi_cfg, package_name, platform, &feature_name);
    }

    #[test]
    fn test_remove_target_dependencies() {
        // Using known files in the project so the test succeed including the file check.
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
            Some(Platform::Linux64),
            &FeatureName::Default,
        );
        test_remove(
            file_contents,
            "bar",
            SpecType::Run,
            Some(Platform::Win64),
            &FeatureName::Default,
        );
        test_remove(
            file_contents,
            "fooz",
            SpecType::Run,
            None,
            &FeatureName::Default,
        );
    }

    #[test]
    fn test_remove_dependencies() {
        // Using known files in the project so the test succeed including the file check.
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
                None,
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
        // Using known files in the project so the test succeed including the file check.
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
        // Using known files in the project so the test succeed including the file check.
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
        // Using known files in the project so the test succeed including the file check.
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
        // Using known files in the project so the test succeed including the file check.
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
    }

    #[test]
    fn test_add_channels() {
        // Using known files in the project so the test succeed including the file check.
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

        let conda_forge = PrioritizedChannel::from_channel(
            Channel::from_str("conda-forge", &channel_config()).unwrap(),
        );
        manifest
            .add_channels([conda_forge.clone()], &FeatureName::Default)
            .unwrap();

        let cuda_feature = FeatureName::Named("cuda".to_string());
        let nvidia = PrioritizedChannel::from_channel(
            Channel::from_str("nvidia", &channel_config()).unwrap(),
        );
        manifest
            .add_channels([nvidia.clone()], &cuda_feature)
            .unwrap();

        let test_feature = FeatureName::Named("test".to_string());
        manifest
            .add_channels(
                [
                    PrioritizedChannel::from_channel(
                        Channel::from_str("test", &channel_config()).unwrap(),
                    ),
                    PrioritizedChannel::from_channel(
                        Channel::from_str("test2", &channel_config()).unwrap(),
                    ),
                ],
                &test_feature,
            )
            .unwrap();

        assert_eq!(
            manifest.parsed.project.channels,
            vec![PrioritizedChannel {
                channel: Channel::from_str("conda-forge", &channel_config()).unwrap(),
                priority: None
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
                channel: Channel::from_str("conda-forge", &channel_config()).unwrap(),
                priority: None
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
                channel: Channel::from_str("nvidia", &channel_config()).unwrap(),
                priority: None
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
                channel: Channel::from_str("nvidia", &channel_config()).unwrap(),
                priority: None
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
                    channel: Channel::from_str("test", &channel_config()).unwrap(),
                    priority: None
                },
                PrioritizedChannel {
                    channel: Channel::from_str("test2", &channel_config()).unwrap(),
                    priority: None
                }
            ]
            .into_iter()
            .collect::<IndexSet<_>>()
        );

        // Test custom channel urls
        let custom_channel = PrioritizedChannel {
            channel: Channel::from_str("https://custom.com/channel", &channel_config()).unwrap(),
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
        // Using known files in the project so the test succeed including the file check.
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
            vec![PrioritizedChannel::from_channel(
                Channel::from_str("conda-forge", &channel_config()).unwrap()
            )]
            .into_iter()
            .collect::<IndexSet<_>>()
        );

        manifest
            .remove_channels(
                [PrioritizedChannel {
                    channel: Channel::from_str("conda-forge", &channel_config()).unwrap(),
                    priority: None,
                }],
                &FeatureName::Default,
            )
            .unwrap();

        assert_eq!(manifest.parsed.project.channels, IndexSet::new());

        manifest
            .remove_channels(
                [PrioritizedChannel {
                    channel: Channel::from_str("test_channel", &channel_config()).unwrap(),
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
                .map(|idx| manifest.parsed.solve_groups.solve_groups[idx].name.as_str()),
            Some("test")
        );

        let test2_env = manifest
            .environment(&EnvironmentName::Named("test2".to_string()))
            .unwrap();
        assert_eq!(test2_env.features, vec!["py39"]);
        assert_eq!(
            test2_env
                .solve_group
                .map(|idx| manifest.parsed.solve_groups.solve_groups[idx].name.as_str()),
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
                    channel: Channel::from_str("pytorch", &channel_config()).unwrap(),
                    priority: None
                },
                &PrioritizedChannel {
                    channel: Channel::from_str("nvidia", &channel_config()).unwrap(),
                    priority: Some(-1)
                }
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
                &MatchSpec::from_str(" baz >=1.2.3", Strict).unwrap(),
                SpecType::Run,
                None,
                &FeatureName::Default,
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
                None,
                &FeatureName::Named("test".to_string()),
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
                Some(Platform::Linux64),
                &FeatureName::Named("extra".to_string()),
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
                Some(Platform::Linux64),
                &FeatureName::Named("build".to_string()),
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
    fn test_duplicate_dependency() {
        let contents = format!(
            r#"
        {PROJECT_BOILERPLATE}

        [dependencies]
        Flask = "2.*"
        flask = "2.*"
        "#
        );
        let manifest = ProjectManifest::from_toml_str(&contents);

        assert!(manifest.is_err());
        assert!(manifest
            .unwrap_err()
            .to_string()
            .contains("duplicate dependency"));
    }

    #[test]
    fn test_duplicate_host_dependency() {
        let contents = format!(
            r#"
        {PROJECT_BOILERPLATE}

        [host-dependencies]
        LibC = "2.12"
        libc = "2.12"
        "#
        );
        let manifest = ProjectManifest::from_toml_str(&contents);

        assert!(manifest.is_err());
        assert!(manifest
            .unwrap_err()
            .to_string()
            .contains("duplicate dependency"));
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
        let _manifest = ProjectManifest::from_toml_str(contents).unwrap();
    }
}
