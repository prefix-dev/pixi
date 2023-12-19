use crate::project::python::PyPiRequirement;
use crate::project::{DependencyType, SpecType};
use crate::task::CmdArgs;
use crate::utils::spanned::PixiSpanned;
use crate::{consts, task::Task};
use ::serde::Deserialize;
use indexmap::IndexMap;
use miette::{Context, IntoDiagnostic, LabeledSpan, NamedSource, Report};
use rattler_conda_types::{
    Channel, ChannelConfig, MatchSpec, NamelessMatchSpec, Platform, Version,
};
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
use rip::types::PackageName;
use serde::Deserializer;
use serde_with::de::DeserializeAsWrap;
use serde_with::{serde_as, DeserializeAs, DisplayFromStr, PickFirst};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use toml_edit::{value, Array, Document, Item, Table, TomlError, Value};
use url::Url;

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
        let (manifest, document) = match toml_edit::de::from_str::<ProjectManifest>(&contents)
            .map_err(TomlError::from)
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
            .pypi_dependencies
            .as_ref()
            .map_or(false, |deps| !deps.is_empty())
        {
            tracing::warn!("ALPHA feature enabled!\n\nIt looks like your project contains `[pypi-dependencies]`. This feature is currently still in an ALPHA state!\n\nYou may encounter bugs or weird behavior. Please report any and all issues you encounter on our github repository:\n\n\thttps://github.com/prefix-dev/pixi.\n");
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

    /// Returns all the targets specific metadata that apply with the given context.
    /// TODO: Add more context here?
    /// TODO: Should we return the selector too to provide better diagnostics later?
    pub fn target_specific_metadata(
        &self,
        platform: Platform,
    ) -> impl Iterator<Item = &'_ TargetMetadata> + '_ {
        self.parsed
            .target
            .iter()
            .filter_map(move |(selector, manifest)| match selector.as_ref() {
                TargetSelector::Platform(p) if p == &platform => Some(manifest),
                _ => None,
            })
    }

    /// Returns a hashmap of the tasks that should run only the given platform.
    pub fn target_specific_tasks(&self, platform: Platform) -> HashMap<&str, &Task> {
        let mut tasks = HashMap::default();
        // Gather platform-specific tasks and overwrite them if they're double.
        for target_metadata in self.target_specific_metadata(platform) {
            tasks.extend(target_metadata.tasks.iter().map(|(k, v)| (k.as_str(), v)));
        }

        tasks
    }

    /// Add a task to the project
    pub fn add_task(
        &mut self,
        name: impl AsRef<str>,
        task: Task,
        platform: Option<Platform>,
    ) -> miette::Result<()> {
        let table = if let Some(platform) = platform {
            if self
                .target_specific_tasks(platform)
                .contains_key(name.as_ref())
            {
                miette::bail!("task {} already exists", name.as_ref());
            }
            ensure_toml_target_table(&mut self.document, platform, "tasks")?
        } else {
            self.document["tasks"]
                .or_insert(Item::Table(Table::new()))
                .as_table_mut()
                .ok_or_else(|| {
                    miette::miette!("target table in {} is malformed", consts::PROJECT_MANIFEST)
                })?
        };
        let depends_on = task.depends_on();

        for depends in depends_on {
            if !self.parsed.tasks.contains_key(depends) {
                miette::bail!(
                    "task '{}' for the depends on for '{}' does not exist",
                    depends,
                    name.as_ref(),
                );
            }
        }

        // Add the task to the table
        table.insert(name.as_ref(), task_as_toml(task.clone()));

        self.parsed.tasks.insert(name.as_ref().to_string(), task);

        self.save()?;

        Ok(())
    }

    /// Returns a hashmap of the tasks that should run on the given platform.
    pub fn tasks(&self, platform: Option<Platform>) -> HashMap<&str, &Task> {
        let mut all_tasks = HashMap::default();

        // Gather non-target specific tasks
        all_tasks.extend(self.parsed.tasks.iter().map(|(k, v)| (k.as_str(), v)));

        // Gather platform-specific tasks and overwrite them if they're double.
        if let Some(platform) = platform {
            for target_metadata in self.target_specific_metadata(platform) {
                all_tasks.extend(target_metadata.tasks.iter().map(|(k, v)| (k.as_str(), v)));
            }
        }

        all_tasks
    }

    /// Remove a task from the project, and the tasks that depend on it
    pub fn remove_task(
        &mut self,
        name: impl AsRef<str>,
        platform: Option<Platform>,
    ) -> miette::Result<()> {
        self.tasks(platform)
            .get(name.as_ref())
            .ok_or_else(|| miette::miette!("task {} does not exist", name.as_ref()))?;

        // Get the task table either from the target platform or the default tasks.
        let tasks_table = if let Some(platform) = platform {
            ensure_toml_target_table(&mut self.document, platform, "tasks")?
        } else {
            let tasks_table = &mut self.document["tasks"];
            if tasks_table.is_none() {
                miette::bail!("internal data-structure inconsistent with toml");
            }
            tasks_table.as_table_like_mut().ok_or_else(|| {
                miette::miette!("tasks in {} are malformed", consts::PROJECT_MANIFEST)
            })?
        };

        // If it does not exist in toml, consider this ok as we want to remove it anyways
        tasks_table.remove(name.as_ref());

        self.save()?;

        Ok(())
    }

    /// Add a platform to the project
    pub fn add_platforms<'a>(
        &mut self,
        platforms: impl Iterator<Item = &'a Platform> + Clone,
    ) -> miette::Result<()> {
        // Add to platform table
        let platform_array = &mut self.document["project"]["platforms"];
        let platform_array = platform_array
            .as_array_mut()
            .expect("platforms should be an array");

        for platform in platforms.clone() {
            platform_array.push(platform.to_string());
        }

        // Add to manifest
        self.parsed.project.platforms.value.extend(platforms);
        Ok(())
    }

    /// Remove the platform(s) from the project
    pub fn remove_platforms(
        &mut self,
        platforms: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> miette::Result<()> {
        let mut removed_platforms = Vec::new();

        for platform in platforms {
            // Parse the channel to be removed
            let platform_to_remove = Platform::from_str(platform.as_ref()).into_diagnostic()?;

            // Remove the channel if it exists
            if let Some(pos) = self
                .parsed
                .project
                .platforms
                .value
                .iter()
                .position(|x| *x == platform_to_remove)
            {
                self.parsed.project.platforms.value.remove(pos);
            }

            removed_platforms.push(platform.as_ref().to_owned());
        }

        // remove the platforms from the toml
        let platform_array = &mut self.document["project"]["platforms"];
        let platform_array = platform_array
            .as_array_mut()
            .expect("platforms should be an array");

        platform_array.retain(|x| !removed_platforms.contains(&x.as_str().unwrap().to_string()));

        Ok(())
    }

    /// Add match spec to the specified table
    fn add_to_deps_table(
        deps_table: &mut Item,
        spec: &MatchSpec,
    ) -> miette::Result<(rattler_conda_types::PackageName, NamelessMatchSpec)> {
        // If it doesn't exist create a proper table
        if deps_table.is_none() {
            *deps_table = Item::Table(Table::new());
        }

        // Cast the item into a table
        let deps_table = deps_table.as_table_like_mut().ok_or_else(|| {
            miette::miette!("dependencies in {} are malformed", consts::PROJECT_MANIFEST)
        })?;

        // Determine the name of the package to add
        let name = spec
            .name
            .clone()
            .ok_or_else(|| miette::miette!("* package specifier is not supported"))?;

        // Format the requirement
        // TODO: Do this smarter. E.g.:
        //  - split this into an object if exotic properties (like channel) are specified.
        //  - split the name from the rest of the requirement.
        let nameless = NamelessMatchSpec::from(spec.to_owned());

        // Store (or replace) in the document
        deps_table.insert(name.as_source(), Item::Value(nameless.to_string().into()));

        Ok((name, nameless))
    }

    /// Add PyPI requirement to the specified table
    fn add_pypi_dep_to_table(
        deps_table: &mut Item,
        name: &rip::types::PackageName,
        requirement: &PyPiRequirement,
    ) -> miette::Result<()> {
        // If it doesn't exist create a proper table
        if deps_table.is_none() {
            *deps_table = Item::Table(Table::new());
        }

        // Cast the item into a table
        let deps_table = deps_table.as_table_like_mut().ok_or_else(|| {
            miette::miette!("dependencies in {} are malformed", consts::PROJECT_MANIFEST)
        })?;

        deps_table.insert(name.as_str(), (*requirement).clone().into());
        Ok(())
    }

    /// Add match spec to the target specific table
    fn add_dep_to_target_table(
        &mut self,
        platform: Platform,
        dep_type: String,
        spec: &MatchSpec,
    ) -> miette::Result<(rattler_conda_types::PackageName, NamelessMatchSpec)> {
        let target = self.document["target"]
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                miette::miette!("target table in {} is malformed", consts::PROJECT_MANIFEST)
            })?;
        target.set_dotted(true);
        let platform_table = self.document["target"][platform.as_str()]
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                miette::miette!(
                    "platform table in {} is malformed",
                    consts::PROJECT_MANIFEST
                )
            })?;
        platform_table.set_dotted(true);

        let dependencies = platform_table[dep_type.as_str()].or_insert(Item::Table(Table::new()));

        Self::add_to_deps_table(dependencies, spec)
    }

    /// Add PyPI requirement to the target specific table
    fn add_pypi_dep_to_target_table(
        &mut self,
        platform: Platform,
        name: &rip::types::PackageName,
        requirement: &PyPiRequirement,
    ) -> miette::Result<()> {
        let target = self.document["target"]
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                miette::miette!("target table in {} is malformed", consts::PROJECT_MANIFEST)
            })?;
        target.set_dotted(true);
        let platform_table = self.document["target"][platform.as_str()]
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                miette::miette!(
                    "platform table in {} is malformed",
                    consts::PROJECT_MANIFEST
                )
            })?;
        platform_table.set_dotted(true);

        let dependencies = platform_table[DependencyType::PypiDependency.name()]
            .or_insert(Item::Table(Table::new()));

        Self::add_pypi_dep_to_table(dependencies, name, requirement)
    }

    pub fn add_target_dependency(
        &mut self,
        platform: Platform,
        spec: &MatchSpec,
        spec_type: SpecType,
    ) -> miette::Result<()> {
        let toml_name = spec_type.name();
        // Add to target table toml
        let (name, nameless) =
            self.add_dep_to_target_table(platform, toml_name.to_string(), spec)?;
        // Add to manifest
        self.parsed
            .target
            .entry(TargetSelector::Platform(platform).into())
            .or_insert(TargetMetadata::default())
            .dependencies
            .insert(name.as_source().into(), nameless);
        Ok(())
    }
    /// Add target specific PyPI requirement to the manifest
    pub fn add_target_pypi_dependency(
        &mut self,
        platform: Platform,
        name: rip::types::PackageName,
        requirement: &PyPiRequirement,
    ) -> miette::Result<()> {
        // Add to target table toml
        self.add_pypi_dep_to_target_table(platform, &name.clone(), requirement)?;
        // Add to manifest
        self.parsed
            .target
            .entry(TargetSelector::Platform(platform).into())
            .or_insert(TargetMetadata::default())
            .pypi_dependencies
            .get_or_insert(IndexMap::new())
            .insert(name, requirement.clone());
        Ok(())
    }

    /// Add a matchspec to the manifest
    pub fn add_dependency(&mut self, spec: &MatchSpec, spec_type: SpecType) -> miette::Result<()> {
        // Find the dependencies table
        let deps = &mut self.document[spec_type.name()];
        let (name, nameless) = Manifest::add_to_deps_table(deps, spec)?;

        self.parsed
            .create_or_get_dependencies(spec_type)
            .insert(name.as_source().into(), nameless);

        Ok(())
    }

    pub fn add_pypi_dependency(
        &mut self,
        name: &rip::types::PackageName,
        requirement: &PyPiRequirement,
    ) -> miette::Result<()> {
        // Find the dependencies table
        let deps = &mut self.document[DependencyType::PypiDependency.name()];
        Manifest::add_pypi_dep_to_table(deps, name, requirement)?;

        self.parsed
            .create_or_get_pypi_dependencies()
            .insert(name.clone(), requirement.clone());

        Ok(())
    }

    /// Removes a dependency from `pixi.toml` based on `SpecType`.
    pub fn remove_dependency(
        &mut self,
        dep: &rattler_conda_types::PackageName,
        spec_type: &SpecType,
    ) -> miette::Result<(String, NamelessMatchSpec)> {
        if let Item::Table(ref mut t) = self.document[spec_type.name()] {
            if t.contains_key(dep.as_normalized()) && t.remove(dep.as_normalized()).is_some() {
                return self
                    .parsed
                    .remove_dependency(dep.as_normalized(), spec_type);
            }
        }

        Err(miette::miette!(
            "Couldn't find {} in [{}]",
            console::style(dep.as_normalized()).bold(),
            console::style(spec_type.name()).bold(),
        ))
    }

    /// Removes a target specific dependency from `pixi.toml` based on `SpecType`.
    pub fn remove_target_dependency(
        &mut self,
        dep: &rattler_conda_types::PackageName,
        spec_type: &SpecType,
        platform: &Platform,
    ) -> miette::Result<(String, NamelessMatchSpec)> {
        let table = get_toml_target_table(&mut self.document, platform, spec_type.name())?;
        table.remove(dep.as_normalized());
        self.parsed
            .remove_target_dependency(dep.as_normalized(), spec_type, platform)
    }

    /// Returns a mutable reference to the channels array.
    fn channels_array_mut(&mut self) -> miette::Result<&mut Array> {
        let project = &mut self.document["project"];
        if project.is_none() {
            *project = Item::Table(Table::new());
        }

        let channels = &mut project["channels"];
        if channels.is_none() {
            *channels = Item::Value(Value::Array(Array::new()))
        }

        channels
            .as_array_mut()
            .ok_or_else(|| miette::miette!("malformed channels array"))
    }

    /// Adds the specified channels to the manifest.
    pub fn add_channels(
        &mut self,
        channels: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> miette::Result<()> {
        let mut stored_channels = Vec::new();
        for channel in channels {
            self.parsed.project.channels.push(
                Channel::from_str(channel.as_ref(), &ChannelConfig::default()).into_diagnostic()?,
            );
            stored_channels.push(channel.as_ref().to_owned());
        }

        let channels_array = self.channels_array_mut()?;
        for channel in stored_channels {
            channels_array.push(channel);
        }

        Ok(())
    }

    /// Remove the specified channels to the manifest.
    pub fn remove_channels(
        &mut self,
        channels: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> miette::Result<()> {
        let mut removed_channels = Vec::new();

        for channel in channels {
            // Parse the channel to be removed
            let channel_to_remove =
                Channel::from_str(channel.as_ref(), &ChannelConfig::default()).into_diagnostic()?;

            // Remove the channel if it exists
            if let Some(pos) = self
                .parsed
                .project
                .channels
                .iter()
                .position(|x| *x == channel_to_remove)
            {
                self.parsed.project.channels.remove(pos);
            }

            removed_channels.push(channel.as_ref().to_owned());
        }

        // remove the channels from the toml
        let channels_array = self.channels_array_mut()?;
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
}

/// Ensures that the specified TOML target table exists within a given document,
/// and inserts it if not.
/// Returns the final target table (`table_name`) inside the platform-specific table if everything
/// goes as expected.
pub fn ensure_toml_target_table<'a>(
    doc: &'a mut Document,
    platform: Platform,
    table_name: &str,
) -> miette::Result<&'a mut Table> {
    // Create target table
    let target = doc["target"]
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| {
            miette::miette!("target table in {} is malformed", consts::PROJECT_MANIFEST)
        })?;
    target.set_dotted(true);

    // Create platform table on target table
    let platform_table = doc["target"][platform.as_str()]
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| {
            miette::miette!(
                "platform table in {} is malformed",
                consts::PROJECT_MANIFEST
            )
        })?;
    platform_table.set_dotted(true);

    // Return final table on target platform table.
    platform_table[table_name]
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| {
            miette::miette!(
                "platform table in {} is malformed",
                consts::PROJECT_MANIFEST
            )
        })
}

/// Retrieve a mutable reference to a target table `table_name`
/// for a specific platform.
fn get_toml_target_table<'a>(
    doc: &'a mut Document,
    platform: &Platform,
    table_name: &str,
) -> miette::Result<&'a mut Table> {
    let platform_table = doc["target"][platform.as_str()]
        .as_table_mut()
        .ok_or(miette::miette!(
            "could not find {} in {}",
            console::style(platform.as_str()).bold(),
            consts::PROJECT_MANIFEST,
        ))?;

    platform_table.set_dotted(true);

    platform_table[table_name]
        .as_table_mut()
        .ok_or(miette::miette!(
            "could not find {} in {}",
            console::style(format!("[target.{}.{}]", platform.as_str(), table_name)).bold(),
            consts::PROJECT_MANIFEST,
        ))
}

/// Describes the contents of a project manifest.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectManifest {
    /// Information about the project
    pub project: ProjectMetadata,

    /// Tasks defined in the project
    #[serde(default)]
    pub tasks: HashMap<String, Task>,

    /// Additional system requirements
    #[serde(default, rename = "system-requirements")]
    pub system_requirements: SystemRequirements,

    /// The dependencies of the project.
    ///
    /// We use an [`IndexMap`] to preserve the order in which the items where defined in the
    /// manifest.
    #[serde(default)]
    #[serde_as(as = "IndexMap<_, PickFirst<(_, DisplayFromStr)>>")]
    pub dependencies: IndexMap<String, NamelessMatchSpec>,

    /// The host-dependencies of the project.
    ///
    /// We use an [`IndexMap`] to preserve the order in which the items where defined in the
    /// manifest.
    #[serde(default, rename = "host-dependencies")]
    #[serde_as(as = "Option<IndexMap<_, PickFirst<(_, DisplayFromStr)>>>")]
    pub host_dependencies: Option<IndexMap<String, NamelessMatchSpec>>,

    /// The build-dependencies of the project.
    ///
    /// We use an [`IndexMap`] to preserve the order in which the items where defined in the
    /// manifest.
    #[serde(default, rename = "build-dependencies")]
    #[serde_as(as = "Option<IndexMap<_, PickFirst<(_, DisplayFromStr)>>>")]
    pub build_dependencies: Option<IndexMap<String, NamelessMatchSpec>>,

    /// Target specific configuration.
    ///
    /// We use an [`IndexMap`] to preserve the order in which the items where defined in the
    /// manifest.
    #[serde(default)]
    pub target: IndexMap<PixiSpanned<TargetSelector>, TargetMetadata>,

    /// Environment activation information.
    ///
    /// We use an [`IndexMap`] to preserve the order in which the items where defined in the
    /// manifest.
    pub activation: Option<Activation>,

    /// Optional python requirements
    #[serde(default, rename = "pypi-dependencies")]
    pub pypi_dependencies: Option<IndexMap<rip::types::PackageName, PyPiRequirement>>,
}

impl ProjectManifest {
    /// Validate the
    pub fn validate(&self, source: NamedSource, root_folder: &Path) -> miette::Result<()> {
        // Check if the targets are defined for existing platforms
        for target_sel in self.target.keys() {
            match target_sel.as_ref() {
                TargetSelector::Platform(p) => {
                    if !self.project.platforms.as_ref().contains(p) {
                        return Err(create_unsupported_platform_report(
                            source,
                            target_sel.span().unwrap_or_default(),
                            p,
                        ));
                    }
                }
            }
        }

        // parse the SPDX license expression to make sure that it is a valid expression.
        if let Some(spdx_expr) = &self.project.license {
            spdx::Expression::parse(spdx_expr)
                .into_diagnostic()
                .with_context(|| {
                    format!(
                        "failed to parse the SPDX license expression '{}'",
                        spdx_expr
                    )
                })?;
        }

        let check_file_existence = |x: &Option<PathBuf>| {
            if let Some(path) = x {
                let full_path = root_folder.join(path);
                if !full_path.exists() {
                    return Err(miette::miette!(
                        "the file '{}' does not exist",
                        full_path.display()
                    ));
                }
            }
            Ok(())
        };

        check_file_existence(&self.project.license_file)?;
        check_file_existence(&self.project.readme)?;

        Ok(())
    }

    /// Get the map of dependencies for a given spec type.
    pub fn create_or_get_dependencies(
        &mut self,
        spec_type: SpecType,
    ) -> &'_ mut IndexMap<String, NamelessMatchSpec> {
        match spec_type {
            SpecType::Run => &mut self.dependencies,
            SpecType::Host => {
                if let Some(ref mut deps) = self.host_dependencies {
                    deps
                } else {
                    self.host_dependencies.insert(IndexMap::new())
                }
            }
            SpecType::Build => {
                if let Some(ref mut deps) = self.build_dependencies {
                    deps
                } else {
                    self.build_dependencies.insert(IndexMap::new())
                }
            }
        }
    }

    /// Get the map of dependencies for a given spec type.
    pub fn create_or_get_pypi_dependencies(
        &mut self,
    ) -> &mut IndexMap<PackageName, PyPiRequirement> {
        if let Some(ref mut deps) = self.pypi_dependencies {
            deps
        } else {
            self.pypi_dependencies.insert(IndexMap::new())
        }
    }

    /// Remove dependency given a `SpecType`.
    pub fn remove_dependency(
        &mut self,
        dep: &str,
        spec_type: &SpecType,
    ) -> miette::Result<(String, NamelessMatchSpec)> {
        let dependencies = match spec_type {
            SpecType::Run => Some(&mut self.dependencies),
            SpecType::Build => self.build_dependencies.as_mut(),
            SpecType::Host => self.host_dependencies.as_mut(),
        };

        if let Some(deps) = dependencies {
            deps.shift_remove_entry(dep).ok_or(miette::miette!(
                "Couldn't find {} in [{}]",
                console::style(dep).bold(),
                console::style(spec_type.name()).bold(),
            ))
        } else {
            Err(miette::miette!(
                "[{}] doesn't exist",
                console::style(spec_type.name()).bold()
            ))
        }
    }

    /// Remove a dependency for a `Platform`.
    pub fn remove_target_dependency(
        &mut self,
        dep: &str,
        spec_type: &SpecType,
        platform: &Platform,
    ) -> miette::Result<(String, NamelessMatchSpec)> {
        let target = PixiSpanned::from(TargetSelector::Platform(*platform));
        let target_metadata = self.target.get_mut(&target).ok_or(miette::miette!(
            "Platform: {} is not configured for this project",
            console::style(platform.as_str()).bold(),
        ))?;

        let dependencies = match spec_type {
            SpecType::Run => Some(&mut target_metadata.dependencies),
            SpecType::Build => target_metadata.build_dependencies.as_mut(),
            SpecType::Host => target_metadata.host_dependencies.as_mut(),
        };

        if let Some(deps) = dependencies {
            deps.shift_remove_entry(dep).ok_or(miette::miette!(
                "Couldn't find {} in [{}]",
                console::style(dep).bold(),
                console::style(format!("target.{}.{}", platform.as_str(), spec_type.name())).bold(),
            ))
        } else {
            Err(miette::miette!(
                "[{}] doesn't exist",
                console::style(format!("target.{}.{}", platform.as_str(), spec_type.name())).bold(),
            ))
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum TargetSelector {
    // Platform specific configuration
    Platform(Platform),
    // TODO: Add minijinja coolness here.
}

struct PlatformTargetSelector;

impl<'de> DeserializeAs<'de, TargetSelector> for PlatformTargetSelector {
    fn deserialize_as<D>(deserializer: D) -> Result<TargetSelector, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(TargetSelector::Platform(Platform::deserialize(
            deserializer,
        )?))
    }
}

impl<'de> Deserialize<'de> for TargetSelector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(
            DeserializeAsWrap::<Self, PickFirst<(PlatformTargetSelector,)>>::deserialize(
                deserializer,
            )?
            .into_inner(),
        )
    }
}

#[serde_as]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetMetadata {
    /// Target specific dependencies
    #[serde(default)]
    #[serde_as(as = "IndexMap<_, PickFirst<(_, DisplayFromStr)>>")]
    pub dependencies: IndexMap<String, NamelessMatchSpec>,

    /// The host-dependencies of the project.
    #[serde(default, rename = "host-dependencies")]
    #[serde_as(as = "Option<IndexMap<_, PickFirst<(_, DisplayFromStr)>>>")]
    pub host_dependencies: Option<IndexMap<String, NamelessMatchSpec>>,

    /// The build-dependencies of the project.
    #[serde(default, rename = "build-dependencies")]
    #[serde_as(as = "Option<IndexMap<_, PickFirst<(_, DisplayFromStr)>>>")]
    pub build_dependencies: Option<IndexMap<String, NamelessMatchSpec>>,

    #[serde(default, rename = "pypi-dependencies")]
    pub pypi_dependencies: Option<IndexMap<rip::types::PackageName, PyPiRequirement>>,

    /// Additional information to activate an environment.
    #[serde(default)]
    pub activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    #[serde(default)]
    pub tasks: HashMap<String, Task>,
}

/// Describes the contents of the `[package]` section of the project manifest.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMetadata {
    /// The name of the project
    pub name: String,

    /// The version of the project
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub version: Option<Version>,

    /// An optional project description
    pub description: Option<String>,

    /// Optional authors
    #[serde(default)]
    pub authors: Vec<String>,

    /// The channels used by the project
    #[serde_as(deserialize_as = "Vec<super::serde::ChannelStr>")]
    pub channels: Vec<Channel>,

    /// The platforms this project supports
    // TODO: This is actually slightly different from the rattler_conda_types::Platform because it
    //     should not include noarch.
    pub platforms: PixiSpanned<Vec<Platform>>,

    /// The license as a valid SPDX string (e.g. MIT AND Apache-2.0)
    pub license: Option<String>,

    /// The license file (relative to the project root)
    #[serde(rename = "license-file")]
    pub license_file: Option<PathBuf>,

    /// Path to the README file of the project (relative to the project root)
    pub readme: Option<PathBuf>,

    /// URL of the project homepage
    pub homepage: Option<Url>,

    /// URL of the project source repository
    pub repository: Option<Url>,

    /// URL of the project documentation
    pub documentation: Option<Url>,
}

#[serde_as]
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SystemRequirements {
    pub windows: Option<bool>,

    pub unix: Option<bool>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    pub macos: Option<Version>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    pub linux: Option<Version>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    pub cuda: Option<Version>,

    pub libc: Option<LibCSystemRequirement>,

    pub archspec: Option<String>,
}

impl SystemRequirements {
    pub fn virtual_packages(&self) -> Vec<VirtualPackage> {
        let mut result = Vec::new();
        if self.windows == Some(true) {
            result.push(VirtualPackage::Win);
        }
        if self.unix == Some(true) {
            result.push(VirtualPackage::Unix);
        }
        if let Some(version) = self.linux.clone() {
            result.push(VirtualPackage::Linux(Linux { version }));
        }
        if let Some(version) = self.cuda.clone() {
            result.push(VirtualPackage::Cuda(Cuda { version }));
        }
        if let Some(version) = self.macos.clone() {
            result.push(VirtualPackage::Osx(Osx { version }))
        }
        if let Some(spec) = self.archspec.clone() {
            result.push(VirtualPackage::Archspec(Archspec { spec }))
        }
        if let Some(libc) = self.libc.clone() {
            result.push(VirtualPackage::LibC(libc.into()))
        }

        result
    }
}

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum LibCSystemRequirement {
    /// Only a version was specified, we assume glibc.
    GlibC(#[serde_as(as = "DisplayFromStr")] Version),

    /// Specified both a family and a version.
    OtherFamily(LibCFamilyAndVersion),
}

impl LibCSystemRequirement {
    /// Returns the family and version of this libc requirement.
    pub fn family_and_version(&self) -> (&str, &Version) {
        match self {
            LibCSystemRequirement::GlibC(version) => ("glibc", version),
            LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion { family, version: v }) => {
                (family.as_deref().unwrap_or("glibc"), v)
            }
        }
    }
}

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibCFamilyAndVersion {
    /// The libc family, e.g. glibc
    pub family: Option<String>,

    /// The minimum version of the libc family
    #[serde_as(as = "DisplayFromStr")]
    pub version: Version,
}

impl From<LibCSystemRequirement> for LibC {
    fn from(value: LibCSystemRequirement) -> Self {
        match value {
            LibCSystemRequirement::GlibC(version) => LibC {
                version,
                family: String::from("glibc"),
            },
            LibCSystemRequirement::OtherFamily(libc) => libc.into(),
        }
    }
}

impl From<LibCFamilyAndVersion> for LibC {
    fn from(value: LibCFamilyAndVersion) -> Self {
        LibC {
            version: value.version,
            family: value.family.unwrap_or_else(|| String::from("glibc")),
        }
    }
}
#[derive(Default, Clone, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Activation {
    pub scripts: Option<Vec<String>>,
}

// Create an error report for using a platform that is not supported by the project.
fn create_unsupported_platform_report(
    source: NamedSource,
    span: Range<usize>,
    platform: &Platform,
) -> Report {
    miette::miette!(
        labels = vec![LabeledSpan::at(
            span,
            format!("'{}' is not a supported platform", platform)
        )],
        help = format!(
            "Add '{platform}' to the `project.platforms` array of the {} manifest.",
            consts::PROJECT_MANIFEST
        ),
        "targeting a platform that this project does not support"
    )
    .with_source_code(source)
}

/// Returns a task as a toml item
fn task_as_toml(task: Task) -> Item {
    match task {
        Task::Plain(str) => Item::Value(str.into()),
        Task::Execute(process) => {
            let mut table = Table::new().into_inline_table();
            match process.cmd {
                CmdArgs::Single(cmd_str) => {
                    table.insert("cmd", cmd_str.into());
                }
                CmdArgs::Multiple(cmd_strs) => {
                    table.insert("cmd", Value::Array(Array::from_iter(cmd_strs)));
                }
            }
            if !process.depends_on.is_empty() {
                table.insert(
                    "depends_on",
                    Value::Array(Array::from_iter(process.depends_on)),
                );
            }
            if let Some(cwd) = process.cwd {
                table.insert("cwd", cwd.to_string_lossy().to_string().into());
            }
            Item::Value(Value::InlineTable(table))
        }
        Task::Alias(alias) => {
            let mut table = Table::new().into_inline_table();
            table.insert(
                "depends_on",
                Value::Array(Array::from_iter(alias.depends_on)),
            );
            Item::Value(Value::InlineTable(table))
        }
        _ => Item::None,
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use insta::{assert_debug_snapshot, assert_display_snapshot};
    use std::str::FromStr;
    use tempfile::tempdir;

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = []
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
        assert_debug_snapshot!(
            toml_edit::de::from_str::<ProjectManifest>(&contents).expect("parsing should succeed!")
        );
    }

    #[test]
    fn test_mapped_dependencies() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [dependencies]
            test_map = {{ version = ">=1.2.3", channel="conda-forge", build="py34_0" }}
            "#
        );

        assert_debug_snapshot!(
            toml_edit::de::from_str::<ProjectManifest>(&contents).expect("parsing should succeed!")
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

        assert_debug_snapshot!(
            toml_edit::de::from_str::<ProjectManifest>(&contents).expect("parsing should succeed!")
        );
    }

    #[test]
    fn test_invalid_target_specific() {
        let examples = [r#"[target.foobar.dependencies]
            invalid_platform = "henk""#];

        assert_display_snapshot!(examples
            .into_iter()
            .map(
                |example| toml_edit::de::from_str::<ProjectManifest>(&format!(
                    "{PROJECT_BOILERPLATE}\n{example}"
                ))
                .unwrap_err()
                .to_string()
            )
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[test]
    fn test_activation_scripts() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [activation]
            scripts = [".pixi/install/setup.sh"]

            [target.win-64.activation]
            scripts = [".pixi/install/setup.ps1"]

            [target.linux-64.activation]
            scripts = [".pixi/install/setup.sh", "test"]
            "#
        );

        assert_debug_snapshot!(
            toml_edit::de::from_str::<ProjectManifest>(&contents).expect("parsing should succeed!")
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

        assert_debug_snapshot!(
            toml_edit::de::from_str::<ProjectManifest>(&contents).expect("parsing should succeed!")
        );
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

        assert_debug_snapshot!(
            toml_edit::de::from_str::<ProjectManifest>(&contents).expect("parsing should succeed!")
        );
    }

    #[test]
    fn system_requirements_works() {
        let file_content = r#"
        windows = true
        unix = true
        linux = "5.11"
        cuda = "12.2"
        macos = "10.15"
        archspec = "arm64"
        libc = { family = "glibc", version = "2.12" }
        "#;

        let system_requirements: SystemRequirements =
            toml_edit::de::from_str(file_content).unwrap();

        let expected_requirements: Vec<VirtualPackage> = vec![
            VirtualPackage::Win,
            VirtualPackage::Unix,
            VirtualPackage::Linux(Linux {
                version: Version::from_str("5.11").unwrap(),
            }),
            VirtualPackage::Cuda(Cuda {
                version: Version::from_str("12.2").unwrap(),
            }),
            VirtualPackage::Osx(Osx {
                version: Version::from_str("10.15").unwrap(),
            }),
            VirtualPackage::Archspec(Archspec {
                spec: "arm64".to_string(),
            }),
            VirtualPackage::LibC(LibC {
                version: Version::from_str("2.12").unwrap(),
                family: "glibc".to_string(),
            }),
        ];

        assert_eq!(
            system_requirements.virtual_packages(),
            expected_requirements
        );
    }

    #[test]
    fn test_system_requirements_failing_edge_cases() {
        let file_contents = [
            r#"
        [system-requirements]
        libc = { verion = "2.12" }
        "#,
            r#"
        [system-requirements]
        lib = "2.12"
        "#,
            r#"
        [system-requirements.libc]
        version = "2.12"
        fam = "glibc"
        "#,
            r#"
        [system-requirements.lic]
        version = "2.12"
        family = "glibc"
        "#,
        ];

        for file_content in file_contents {
            let file_content = format!("{PROJECT_BOILERPLATE}\n{file_content}");
            assert!(toml_edit::de::from_str::<ProjectManifest>(&file_content).is_err());
        }
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

        let tmpdir = tempdir().unwrap();

        let mut manifest = Manifest::from_str(tmpdir.path(), file_contents).unwrap();

        manifest
            .parsed
            .remove_target_dependency("baz", &SpecType::Build, &Platform::Linux64)
            .unwrap();
        assert_debug_snapshot!(manifest.parsed);
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
                &rattler_conda_types::PackageName::try_from("fooz").unwrap(),
                &SpecType::Run,
            )
            .unwrap();
        assert!(manifest.parsed.dependencies.is_empty());
        // Should still contain the fooz dependency in the target table
        assert_debug_snapshot!(manifest.parsed.target);
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

            [dependencies]
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
}
