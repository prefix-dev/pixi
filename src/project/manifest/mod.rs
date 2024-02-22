mod activation;
pub(crate) mod channel;
mod environment;
mod error;
mod feature;
mod metadata;
mod python;
mod system_requirements;
mod target;
mod validation;

use crate::project::manifest::channel::PrioritizedChannel;
use crate::project::manifest::environment::TomlEnvironmentMapOrSeq;
use crate::task::TaskName;
use crate::{consts, project::SpecType, task::Task, utils::spanned::PixiSpanned};
pub use activation::Activation;
pub use environment::{Environment, EnvironmentName};
pub use feature::{Feature, FeatureName};
use indexmap::map::Entry;
use indexmap::{Equivalent, IndexMap, IndexSet};
use itertools::Itertools;
pub use metadata::ProjectMetadata;
use miette::{miette, Diagnostic, IntoDiagnostic, LabeledSpan, NamedSource};
pub use python::PyPiRequirement;
use rattler_conda_types::{MatchSpec, NamelessMatchSpec, PackageName, Platform, Version};
use serde::de::{DeserializeSeed, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_with::serde_as;
use std::fmt;
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
use toml_edit::{value, Array, Document, Item, Table, TomlError, Value};

/// Errors that can occur when getting a feature.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum GetFeatureError {
    #[error("feature `{0}` does not exist")]
    FeatureDoesNotExist(FeatureName),
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
    pub document: toml_edit::Document,

    /// The parsed manifest
    pub parsed: ProjectManifest,
}

impl Manifest {
    /// Create a new manifest from a path
    pub fn from_path(path: impl AsRef<Path>) -> miette::Result<Self> {
        let contents = std::fs::read_to_string(path.as_ref()).into_diagnostic()?;
        let parent = path
            .as_ref()
            .parent()
            .expect("Path should always have a parent");
        Self::from_str(parent, contents)
    }

    /// Create a new manifest from a string
    pub fn from_str(root: &Path, contents: impl Into<String>) -> miette::Result<Self> {
        let contents = contents.into();
        let (manifest, document) = match ProjectManifest::from_toml_str(&contents)
            .and_then(|manifest| contents.parse::<Document>().map(|doc| (manifest, doc)))
        {
            Ok(result) => result,
            Err(e) => {
                if let Some(span) = e.span() {
                    return Err(miette::miette!(
                        labels = vec![LabeledSpan::at(span, e.message())],
                        "failed to parse project manifest"
                    )
                    .with_source_code(NamedSource::new(consts::PROJECT_MANIFEST, contents)));
                } else {
                    return Err(e).into_diagnostic();
                }
            }
        };

        // Validate the contents of the manifest
        manifest.validate(
            NamedSource::new(consts::PROJECT_MANIFEST, contents.to_owned()),
            root,
        )?;

        // Notify the user that pypi-dependencies are still experimental
        if manifest
            .features
            .values()
            .flat_map(|f| f.targets.targets())
            .any(|f| f.pypi_dependencies.is_some())
        {
            match std::env::var("PIXI_BETA_WARNING_OFF") {
                Ok(var) if var == *"true" => {}
                _ => {
                    tracing::warn!("BETA feature `[pypi-dependencies]` enabled!\n\nPlease report any and all issues here:\n\n\thttps://github.com/prefix-dev/pixi.\n\nTurn this warning off by setting the environment variable `PIXI_BETA_WARNING_OFF` to `true`.\n");
                }
            }
        }

        Ok(Self {
            path: root.join(consts::PROJECT_MANIFEST),
            contents,
            document,
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

        // Get the table that contains the tasks.
        let table = get_or_insert_toml_table(&mut self.document, platform, feature_name, "tasks")?;

        // Add the task to the table
        table.insert(name.as_str(), task.clone().into());

        // Add the task to the manifest
        self.default_feature_mut()
            .targets
            .for_opt_target_or_default_mut(platform.map(TargetSelector::from).as_ref())
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
        self.tasks(platform, feature_name)?
            .get(&name)
            .ok_or_else(|| miette::miette!("task {} does not exist", name.fancy_display()))?;

        // Get the task table either from the target platform or the default tasks.
        let tasks_table =
            get_or_insert_toml_table(&mut self.document, platform, feature_name, "tasks")?;

        // If it does not exist in toml, consider this ok as we want to remove it anyways
        tasks_table.remove(name.as_str());

        // Remove the task from the internal manifest
        self.feature_mut(feature_name)
            .expect("feature should exist")
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
        let mut stored_platforms = IndexSet::new();
        match feature_name {
            FeatureName::Default => {
                for platform in platforms {
                    // TODO: Make platforms a IndexSet to avoid duplicates.
                    if self
                        .parsed
                        .project
                        .platforms
                        .value
                        .iter()
                        .any(|x| x == platform)
                    {
                        continue;
                    }
                    self.parsed.project.platforms.value.push(*platform);

                    stored_platforms.insert(platform);
                }
            }
            FeatureName::Named(_) => {
                for platform in platforms {
                    match self.parsed.features.entry(feature_name.clone()) {
                        Entry::Occupied(mut entry) => {
                            if let Some(platforms) = &mut entry.get_mut().platforms {
                                if platforms.value.iter().any(|x| x == platform) {
                                    continue;
                                }
                            }
                            // If the feature already exists, just push the new platform
                            entry
                                .get_mut()
                                .platforms
                                .get_or_insert_with(Default::default)
                                .value
                                .push(*platform);
                        }
                        Entry::Vacant(entry) => {
                            // If the feature does not exist, insert a new feature with the new platform
                            entry.insert(Feature {
                                name: feature_name.clone(),
                                platforms: Some(PixiSpanned::from(vec![*platform])),
                                system_requirements: Default::default(),
                                targets: Default::default(),
                                channels: None,
                            });
                        }
                    }
                    stored_platforms.insert(platform);
                }
            }
        }
        // Then add the platforms to the toml document
        let platforms_array = self.specific_array_mut("platforms", feature_name)?;
        for platform in stored_platforms {
            platforms_array.push(platform.to_string());
        }

        Ok(())
    }

    /// Remove the platform(s) from the project
    pub fn remove_platforms(
        &mut self,
        platforms: &Vec<Platform>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        let mut removed_platforms = Vec::new();
        match feature_name {
            FeatureName::Default => {
                for platform in platforms {
                    if let Some(index) = self
                        .parsed
                        .project
                        .platforms
                        .value
                        .iter()
                        .position(|x| x == platform)
                    {
                        self.parsed.project.platforms.value.remove(index);
                        removed_platforms.push(platform.to_string());
                    }
                }
            }
            FeatureName::Named(_) => {
                for platform in platforms {
                    match self.parsed.features.entry(feature_name.clone()) {
                        Entry::Occupied(mut entry) => {
                            if let Some(platforms) = &mut entry.get_mut().platforms {
                                if let Some(index) =
                                    platforms.value.iter().position(|x| x == platform)
                                {
                                    platforms.value.remove(index);
                                }
                            }
                        }
                        Entry::Vacant(_entry) => {
                            return Err(miette!(
                                "Feature {} does not exist",
                                feature_name.as_str()
                            ));
                        }
                    }
                    removed_platforms.push(platform.to_string());
                }
            }
        }

        // remove the channels from the toml
        let platforms_array = self.specific_array_mut("platforms", feature_name)?;
        platforms_array.retain(|x| !removed_platforms.contains(&x.as_str().unwrap().to_string()));

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
        // Find the table toml table to add the dependency to.
        let dependency_table =
            get_or_insert_toml_table(&mut self.document, platform, feature_name, spec_type.name())?;

        // Determine the name of the package to add
        let (Some(name), spec) = spec.clone().into_nameless() else {
            miette::bail!("pixi does not support wildcard dependencies")
        };

        // Store (or replace) in the document
        dependency_table.insert(name.as_source(), Item::Value(spec.to_string().into()));

        // Add the dependency to the manifest as well
        self.parsed
            .features
            .entry(feature_name.clone())
            .or_default()
            .targets
            .for_opt_target_or_default_mut(platform.map(TargetSelector::from).as_ref())
            .dependencies
            .entry(spec_type)
            .or_default()
            .insert(name, spec);

        Ok(())
    }

    pub fn add_pypi_dependency(
        &mut self,
        name: &rip::types::PackageName,
        requirement: &PyPiRequirement,
        platform: Option<Platform>,
    ) -> miette::Result<()> {
        // Find the table toml table to add the dependency to.
        let dependency_table = get_or_insert_toml_table(
            &mut self.document,
            platform,
            &FeatureName::Default,
            consts::PYPI_DEPENDENCIES,
        )?;

        // Add the pypi dependency to the table
        dependency_table.insert(name.as_str(), (*requirement).clone().into());

        // Add the dependency to the manifest as well
        self.default_feature_mut()
            .targets
            .for_opt_target_or_default_mut(platform.map(TargetSelector::from).as_ref())
            .pypi_dependencies
            .get_or_insert_with(Default::default)
            .insert(name.clone(), requirement.clone());

        Ok(())
    }

    /// Removes a dependency from `pixi.toml` based on `SpecType`.
    pub fn remove_dependency(
        &mut self,
        dep: &PackageName,
        spec_type: SpecType,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> miette::Result<(PackageName, NamelessMatchSpec)> {
        get_or_insert_toml_table(&mut self.document, platform, feature_name, spec_type.name())?
            .remove(dep.as_source())
            .ok_or_else(|| {
                let table_name =
                    get_nested_toml_table_name(feature_name, platform, spec_type.name());
                miette::miette!(
                    "Couldn't find {} in [{}]",
                    console::style(dep.as_source()).bold(),
                    console::style(table_name).bold(),
                )
            })?;

        Ok(self
            .parsed
            .features
            .get_mut(feature_name)
            .expect("feature should exist")
            .targets
            .for_opt_target_mut(platform.map(TargetSelector::Platform).as_ref())
            .expect("target should exist")
            .remove_dependency(dep.as_source(), spec_type)
            .expect("dependency should exist"))
    }

    /// Removes a pypi dependency from `pixi.toml`.
    pub fn remove_pypi_dependency(
        &mut self,
        dep: &rip::types::PackageName,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> miette::Result<(rip::types::PackageName, PyPiRequirement)> {
        get_or_insert_toml_table(
            &mut self.document,
            platform,
            feature_name,
            consts::PYPI_DEPENDENCIES,
        )?
        .remove(dep.as_source_str())
        .ok_or_else(|| {
            let table_name =
                get_nested_toml_table_name(feature_name, platform, consts::PYPI_DEPENDENCIES);

            miette::miette!(
                "Couldn't find {} in [{}]",
                console::style(dep.as_source_str()).bold(),
                console::style(table_name).bold(),
            )
        })?;

        Ok(self
            .parsed
            .features
            .get_mut(feature_name)
            .expect("feature should exist")
            .targets
            .for_opt_target_mut(platform.map(TargetSelector::Platform).as_ref())
            .expect("target should exist")
            .pypi_dependencies
            .as_mut()
            .expect("pypi-dependencies should exist")
            .shift_remove_entry(dep)
            .expect("dependency should exist"))
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

    /// Returns a mutable reference to the specified array either in project or feature.
    fn specific_array_mut(
        &mut self,
        array_name: &str,
        feature_name: &FeatureName,
    ) -> miette::Result<&mut Array> {
        match feature_name {
            FeatureName::Default => {
                let project = &mut self.document["project"];
                if project.is_none() {
                    *project = Item::Table(Table::new());
                }

                let channels = &mut project[array_name];
                if channels.is_none() {
                    *channels = Item::Value(Value::Array(Array::new()))
                }

                channels
                    .as_array_mut()
                    .ok_or_else(|| miette::miette!("malformed {array_name} array"))
            }
            FeatureName::Named(_) => {
                let feature = &mut self.document["feature"];
                if feature.is_none() {
                    *feature = Item::Table(Table::new());
                }
                let table = feature.as_table_mut().expect("feature should be a table");
                table.set_dotted(true);

                let feature = &mut table[feature_name.as_str()];
                if feature.is_none() {
                    *feature = Item::Table(Table::new());
                }

                let channels = &mut feature[array_name];
                if channels.is_none() {
                    *channels = Item::Value(Value::Array(Array::new()))
                }

                channels
                    .as_array_mut()
                    .ok_or_else(|| miette::miette!("malformed {array_name} array"))
            }
        }
    }

    /// Adds the specified channels to the manifest.
    pub fn add_channels(
        &mut self,
        channels: impl IntoIterator<Item = PrioritizedChannel>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        // First add the channels to the manifest
        let mut stored_channels = IndexSet::new();
        match feature_name {
            FeatureName::Default => {
                for channel in channels {
                    // TODO: Make channels a IndexSet to avoid duplicates.
                    if self.parsed.project.channels.iter().any(|x| x == &channel) {
                        continue;
                    }
                    self.parsed.project.channels.push(channel.clone());

                    stored_channels.insert(channel.channel.name().to_string());
                }
            }
            FeatureName::Named(_) => {
                for channel in channels {
                    match self.parsed.features.entry(feature_name.clone()) {
                        Entry::Occupied(mut entry) => {
                            if let Some(channels) = &mut entry.get_mut().channels {
                                if channels.iter().any(|x| x == &channel) {
                                    continue;
                                }
                            }
                            // If the feature already exists, just push the new channel
                            entry
                                .get_mut()
                                .channels
                                .get_or_insert_with(Vec::new)
                                .push(channel.clone());
                        }
                        Entry::Vacant(entry) => {
                            // If the feature does not exist, insert a new feature with the new channel
                            entry.insert(Feature {
                                name: feature_name.clone(),
                                platforms: None,
                                channels: Some(vec![channel.clone()]),
                                system_requirements: Default::default(),
                                targets: Default::default(),
                            });
                        }
                    }
                    stored_channels.insert(channel.channel.name().to_string());
                }
            }
        }
        // Then add the channels to the toml document
        let channels_array = self.specific_array_mut("channels", feature_name)?;
        for channel in stored_channels {
            channels_array.push(channel);
        }

        Ok(())
    }

    /// Remove the specified channels to the manifest.
    pub fn remove_channels(
        &mut self,
        channels: impl IntoIterator<Item = PrioritizedChannel>,
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        let mut removed_channels = Vec::new();

        match feature_name {
            FeatureName::Default => {
                for channel in channels {
                    // TODO: Make channels a IndexSet to simplify this.
                    if self.parsed.project.channels.iter().any(|x| x == &channel) {
                        if let Some(index) = self
                            .parsed
                            .project
                            .channels
                            .iter()
                            .position(|x| *x == channel)
                        {
                            self.parsed.project.channels.remove(index);
                        }
                        removed_channels.push(channel.channel.name().to_string());
                    }
                }
            }
            FeatureName::Named(_) => {
                for channel in channels {
                    match self.parsed.features.entry(feature_name.clone()) {
                        Entry::Occupied(mut entry) => {
                            if let Some(channels) = &mut entry.get_mut().channels {
                                if let Some(index) = channels.iter().position(|x| *x == channel) {
                                    channels.remove(index);
                                }
                            }
                        }
                        Entry::Vacant(_entry) => {
                            return Err(miette!(
                                "Feature {} does not exist",
                                feature_name.as_str()
                            ));
                        }
                    }
                    removed_channels.push(channel.channel.name().to_string());
                }
            }
        }

        // remove the channels from the toml
        let channels_array = self.specific_array_mut("channels", feature_name)?;
        channels_array.retain(|x| !removed_channels.contains(&x.as_str().unwrap().to_string()));

        Ok(())
    }

    /// Set the project description
    pub fn set_description(&mut self, description: &String) -> miette::Result<()> {
        // Update in both the manifest and the toml
        self.parsed.project.description = Some(description.to_string());
        self.document["project"]["description"] = value(description);

        Ok(())
    }

    /// Set the project version
    pub fn set_version(&mut self, version: &String) -> miette::Result<()> {
        // Update in both the manifest and the toml
        self.parsed.project.version = Some(Version::from_str(version).unwrap());
        self.document["project"]["version"] = value(version);

        Ok(())
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
    pub fn feature_mut<Q: ?Sized>(&mut self, name: &Q) -> Option<&mut Feature>
    where
        Q: Hash + Equivalent<FeatureName>,
    {
        self.parsed.features.get_mut(name)
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

/// Returns the name of a nested TOML table.
/// If `platform` and `feature_name` are `None`, the table name is returned as-is.
/// Otherwise, the table name is prefixed with the feature, platform, or both.
fn get_nested_toml_table_name(
    feature_name: &FeatureName,
    platform: Option<Platform>,
    table: &str,
) -> String {
    match (platform, feature_name) {
        (Some(platform), FeatureName::Named(_)) => format!(
            "feature.{}.target.{}.{}",
            feature_name.as_str(),
            platform.as_str(),
            table
        ),
        (Some(platform), FeatureName::Default) => {
            format!("target.{}.{}", platform.as_str(), table)
        }
        (None, FeatureName::Named(_)) => {
            format!("feature.{}.{}", feature_name.as_str(), table)
        }
        (None, FeatureName::Default) => table.to_string(),
    }
}

/// Retrieve a mutable reference to a target table `table_name`
/// for a specific platform.
/// If table not found, its inserted into the document.
fn get_or_insert_toml_table<'a>(
    doc: &'a mut Document,
    platform: Option<Platform>,
    feature: &FeatureName,
    table_name: &str,
) -> miette::Result<&'a mut Table> {
    let table_name = get_nested_toml_table_name(feature, platform, table_name);
    let parts: Vec<&str> = table_name.split('.').collect();

    let mut current_table = doc.as_table_mut();
    for (i, part) in parts.iter().enumerate() {
        current_table = current_table
            .entry(part)
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                miette!(
                    "Could not find or access the part '{}' in the path '[{}]'",
                    part,
                    table_name
                )
            })?;
        if i < parts.len() - 1 {
            current_table.set_dotted(true);
        }
    }
    Ok(current_table)
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
        toml_edit::de::from_str(source).map_err(TomlError::from)
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
            .string(|str| NamelessMatchSpec::from_str(str).map_err(serde::de::Error::custom))
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
            pypi_dependencies: Option<IndexMap<rip::types::PackageName, PyPiRequirement>>,

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
            environments.environments.push(Environment {
                name: EnvironmentName::Default,
                features: Vec::new(),
                features_source_loc: None,
                solve_group: None,
            });
            environments.by_name.insert(EnvironmentName::Default, 0);
        }

        // Add all named environments
        for (name, env) in toml_manifest.environments {
            let environment_idx = environments.environments.len();
            environments.by_name.insert(name.clone(), environment_idx);

            // Decompose the TOML
            let (features, features_source_loc, solve_group) = match env {
                TomlEnvironmentMapOrSeq::Map(env) => {
                    (env.features.value, env.features.span, env.solve_group)
                }
                TomlEnvironmentMapOrSeq::Seq(features) => (features, None, None),
            };

            // Add to the solve group if defined
            let solve_group = if let Some(solve_group) = solve_group {
                Some(match solve_groups.by_name.get(&solve_group) {
                    Some(idx) => {
                        solve_groups.solve_groups[*idx]
                            .environments
                            .push(environment_idx);
                        *idx
                    }
                    None => {
                        let idx = solve_groups.solve_groups.len();
                        solve_groups.solve_groups.push(SolveGroup {
                            name: solve_group.clone(),
                            environments: vec![environment_idx],
                        });
                        solve_groups.by_name.insert(solve_group, idx);
                        idx
                    }
                })
            } else {
                None
            };

            environments.environments.push(Environment {
                name,
                features,
                features_source_loc,
                solve_group,
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
    use crate::project::manifest::channel::PrioritizedChannel;
    use insta::assert_display_snapshot;
    use rattler_conda_types::{Channel, ChannelConfig};
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

        assert_eq!(manifest.parsed.project.name, "foo");
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

        assert_display_snapshot!(examples
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
        assert_display_snapshot!(examples
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

        let manifest = Manifest::from_str(Path::new(""), contents).unwrap();
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

        assert_display_snapshot!(manifest
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

        assert_display_snapshot!(toml_edit::de::from_str::<ProjectManifest>(&contents)
            .expect("parsing should succeed!")
            .default_feature()
            .targets
            .default()
            .pypi_dependencies
            .clone()
            .into_iter()
            .flat_map(|d| d.into_iter())
            .map(|(name, spec)| format!("{} = {}", name.as_source_str(), Item::from(spec)))
            .join("\n"));
    }

    fn test_remove(
        file_contents: &str,
        name: &str,
        kind: SpecType,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) {
        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        // Initially the dependency should exist
        assert!(manifest
            .feature_mut(feature_name)
            .expect(&format!("feature `{}` should exist", feature_name.as_str()))
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
            .expect(&format!("feature `{}` should exist", feature_name.as_str()))
            .targets
            .for_opt_target(platform.map(TargetSelector::Platform).as_ref())
            .unwrap()
            .dependencies
            .get(&kind)
            .unwrap()
            .get(name)
            .is_none());

        // Write the toml to string and verify the content
        assert_display_snapshot!(
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
        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        let package_name = rip::types::PackageName::from_str(name).unwrap();

        // Initially the dependency should exist
        assert!(manifest
            .feature_mut(feature_name)
            .expect(&format!("feature `{}` should exist", feature_name.as_str()))
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
            .expect(&format!("feature `{}` should exist", feature_name.as_str()))
            .targets
            .for_opt_target(platform.map(TargetSelector::Platform).as_ref())
            .unwrap()
            .pypi_dependencies
            .as_ref()
            .unwrap()
            .get(&package_name)
            .is_none());

        // Write the toml to string and verify the content
        assert_display_snapshot!(
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

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

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

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

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

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

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

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        assert_eq!(
            manifest.parsed.project.platforms.value,
            vec![Platform::Linux64, Platform::Win64]
        );

        manifest
            .add_platforms([Platform::OsxArm64].iter(), &FeatureName::Default)
            .unwrap();

        assert_eq!(
            manifest.parsed.project.platforms.value,
            vec![Platform::Linux64, Platform::Win64, Platform::OsxArm64]
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

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        assert_eq!(
            manifest.parsed.project.platforms.value,
            vec![Platform::Linux64, Platform::Win64]
        );

        manifest
            .remove_platforms(&vec![Platform::Linux64], &FeatureName::Default)
            .unwrap();

        assert_eq!(
            manifest.parsed.project.platforms.value,
            vec![Platform::Win64]
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
        );

        manifest
            .remove_platforms(
                &vec![Platform::Linux64, Platform::Osx64],
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
            vec![Platform::Win64]
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

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        assert_eq!(manifest.parsed.project.channels, vec![]);

        let conda_forge = PrioritizedChannel::from_channel(
            Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap(),
        );
        manifest
            .add_channels([conda_forge.clone()].into_iter(), &FeatureName::Default)
            .unwrap();

        let cuda_feature = FeatureName::Named("cuda".to_string());
        let nvidia = PrioritizedChannel::from_channel(
            Channel::from_str("nvidia", &ChannelConfig::default()).unwrap(),
        );
        manifest
            .add_channels([nvidia.clone()].into_iter(), &cuda_feature)
            .unwrap();

        let test_feature = FeatureName::Named("test".to_string());
        manifest
            .add_channels(
                [
                    PrioritizedChannel::from_channel(
                        Channel::from_str("test", &ChannelConfig::default()).unwrap(),
                    ),
                    PrioritizedChannel::from_channel(
                        Channel::from_str("test2", &ChannelConfig::default()).unwrap(),
                    ),
                ]
                .into_iter(),
                &test_feature,
            )
            .unwrap();

        assert_eq!(
            manifest.parsed.project.channels,
            vec![PrioritizedChannel {
                channel: Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap(),
                priority: None
            }]
        );

        // Try to add again, should not add more channels
        manifest
            .add_channels([conda_forge.clone()].into_iter(), &FeatureName::Default)
            .unwrap();

        assert_eq!(
            manifest.parsed.project.channels,
            vec![PrioritizedChannel {
                channel: Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap(),
                priority: None
            }]
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
                channel: Channel::from_str("nvidia", &ChannelConfig::default()).unwrap(),
                priority: None
            }]
        );
        // Try to add again, should not add more channels
        manifest
            .add_channels([nvidia.clone()].into_iter(), &cuda_feature)
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
                channel: Channel::from_str("nvidia", &ChannelConfig::default()).unwrap(),
                priority: None
            }]
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
                    channel: Channel::from_str("test", &ChannelConfig::default()).unwrap(),
                    priority: None
                },
                PrioritizedChannel {
                    channel: Channel::from_str("test2", &ChannelConfig::default()).unwrap(),
                    priority: None
                }
            ]
        );

        assert_display_snapshot!(manifest.document.to_string());
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

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        assert_eq!(
            manifest.parsed.project.channels,
            vec![PrioritizedChannel::from_channel(
                Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap()
            )]
        );

        manifest
            .remove_channels(
                [PrioritizedChannel {
                    channel: Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap(),
                    priority: None,
                }]
                .into_iter(),
                &FeatureName::Default,
            )
            .unwrap();

        assert_eq!(manifest.parsed.project.channels, vec![]);

        manifest
            .remove_channels(
                [PrioritizedChannel {
                    channel: Channel::from_str("test_channel", &ChannelConfig::default()).unwrap(),
                    priority: None,
                }]
                .into_iter(),
                &FeatureName::Named("test".to_string()),
            )
            .unwrap();

        let feature_channels = manifest
            .feature(&FeatureName::Named("test".to_string()))
            .unwrap()
            .channels
            .clone()
            .unwrap();
        assert_eq!(feature_channels, vec![]);
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
        let manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();
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
        let manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

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
                .get(
                    &rip::types::PackageName::from_str("torch")
                        .expect("torch should be a valid name")
                )
                .expect("pypi requirement should be available")
                .version
                .clone()
                .unwrap()
                .to_string(),
            "~=1.9.0"
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
                    channel: Channel::from_str("pytorch", &ChannelConfig::default()).unwrap(),
                    priority: None
                },
                &PrioritizedChannel {
                    channel: Channel::from_str("nvidia", &ChannelConfig::default()).unwrap(),
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
            Path::new(""),
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

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

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
        assert_display_snapshot!(manifest.document.to_string());
    }

    #[test]
    fn test_get_nested_toml_table_name() {
        // Test all different options for the feature name and platform
        assert_eq!(
            "dependencies".to_string(),
            get_nested_toml_table_name(&FeatureName::Default, None, "dependencies")
        );
        assert_eq!(
            "target.linux-64.dependencies".to_string(),
            get_nested_toml_table_name(
                &FeatureName::Default,
                Some(Platform::Linux64),
                "dependencies"
            )
        );
        assert_eq!(
            "feature.test.dependencies".to_string(),
            get_nested_toml_table_name(
                &FeatureName::Named("test".to_string()),
                None,
                "dependencies"
            )
        );
        assert_eq!(
            "feature.test.target.linux-64.dependencies".to_string(),
            get_nested_toml_table_name(
                &FeatureName::Named("test".to_string()),
                Some(Platform::Linux64),
                "dependencies"
            )
        );
    }

    #[test]
    fn test_get_or_insert_toml_table() {
        let mut manifest = Manifest::from_str(Path::new(""), PROJECT_BOILERPLATE).unwrap();
        let _ =
            get_or_insert_toml_table(&mut manifest.document, None, &FeatureName::Default, "tasks");
        let _ = get_or_insert_toml_table(
            &mut manifest.document,
            Some(Platform::Linux64),
            &FeatureName::Default,
            "tasks",
        );
        let _ = get_or_insert_toml_table(
            &mut manifest.document,
            None,
            &FeatureName::Named("test".to_string()),
            "tasks",
        );
        let _ = get_or_insert_toml_table(
            &mut manifest.document,
            Some(Platform::Linux64),
            &FeatureName::Named("test".to_string()),
            "tasks",
        );
        assert_display_snapshot!(manifest.document.to_string());
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
        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();
        manifest
            .add_dependency(
                &MatchSpec::from_str(" baz >=1.2.3").unwrap(),
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
                &MatchSpec::from_str(" bal >=2.3").unwrap(),
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
                &MatchSpec::from_str(" boef >=2.3").unwrap(),
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
                &MatchSpec::from_str(" cmake >=2.3").unwrap(),
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

        assert_display_snapshot!(manifest.document.to_string());
    }
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
}
