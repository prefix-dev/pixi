pub mod environment;
pub mod manifest;
mod python;
mod serde;

use indexmap::IndexMap;
use miette::{IntoDiagnostic, LabeledSpan, NamedSource, WrapErr};
use once_cell::sync::OnceCell;
use rattler_conda_types::{
    Channel, ChannelConfig, MatchSpec, NamelessMatchSpec, PackageName, Platform, Version,
};
use rattler_virtual_packages::VirtualPackage;
use rip::{normalize_index_url, PackageDb};
use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::project::python::PyPiRequirement;
use crate::{
    consts::{self, PROJECT_MANIFEST},
    default_client,
    project::manifest::{ProjectManifest, TargetMetadata, TargetSelector},
    task::{CmdArgs, Task},
    virtual_packages::non_relevant_virtual_packages_for_platform,
};
use toml_edit::{Array, Document, Item, Table, TomlError, Value};
use url::Url;

#[derive(Debug, Copy, Clone)]
/// What kind of dependency spec do we have
pub enum SpecType {
    /// Host dependencies are used that are needed by the host environment when running the project
    Host,
    /// Build dependencies are used when we need to build the project, may not be required at runtime
    Build,
    /// Regular dependencies that are used when we need to run the project
    Run,
}

impl SpecType {
    /// Convert to a name used in the manifest
    pub fn name(&self) -> &'static str {
        match self {
            SpecType::Host => "host-dependencies",
            SpecType::Build => "build-dependencies",
            SpecType::Run => "dependencies",
        }
    }
}

/// A project represented by a pixi.toml file.
#[derive(Clone)]
pub struct Project {
    root: PathBuf,
    source: String,
    doc: Document,
    package_db: OnceCell<Arc<PackageDb>>,
    pub manifest: ProjectManifest,
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
                    table.insert("cmd", Value::Array(Array::from_iter(cmd_strs.into_iter())));
                }
            }
            if !process.depends_on.is_empty() {
                table.insert(
                    "depends_on",
                    Value::Array(Array::from_iter(process.depends_on.into_iter())),
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
                Value::Array(Array::from_iter(alias.depends_on.into_iter())),
            );
            Item::Value(Value::InlineTable(table))
        }
        _ => Item::None,
    }
}

impl Project {
    /// Constructs a new instance from an internal manifest representation
    pub fn from_manifest(manifest: ProjectManifest) -> Self {
        Self {
            root: Default::default(),
            source: "".to_string(),
            doc: Default::default(),
            package_db: Default::default(),
            manifest,
        }
    }

    /// Discovers the project manifest file in the current directory or any of the parent
    /// directories.
    /// This will also set the current working directory to the project root.
    pub fn discover() -> miette::Result<Self> {
        let project_toml = match find_project_root() {
            Some(root) => root.join(consts::PROJECT_MANIFEST),
            None => miette::bail!("could not find {}", consts::PROJECT_MANIFEST),
        };
        Self::load(&project_toml)
    }

    /// Returns the source code of the project as [`NamedSource`].
    pub fn source(&self) -> NamedSource {
        NamedSource::new(consts::PROJECT_MANIFEST, self.source.clone())
    }

    /// Loads a project manifest file.
    pub fn load(filename: &Path) -> miette::Result<Self> {
        // Determine the parent directory of the manifest file
        let full_path = dunce::canonicalize(filename).into_diagnostic()?;
        if full_path.file_name().and_then(OsStr::to_str) != Some(PROJECT_MANIFEST) {
            miette::bail!("the manifest-path must point to a {PROJECT_MANIFEST} file");
        }

        let root = full_path
            .parent()
            .ok_or_else(|| miette::miette!("can not find parent of {}", filename.display()))?;

        // Load the TOML document
        fs::read_to_string(filename)
            .into_diagnostic()
            .and_then(|content| Self::from_manifest_str(root, content))
            .wrap_err_with(|| {
                format!(
                    "failed to parse {} from {}",
                    consts::PROJECT_MANIFEST,
                    root.display()
                )
            })
    }

    /// Returns all tasks defined in the project for the given platform
    pub fn task_names(&self, platform: Option<Platform>) -> Vec<&String> {
        let mut all_tasks = HashSet::new();

        // Get all non-target specific tasks
        all_tasks.extend(self.manifest.tasks.keys());

        // Gather platform-specific tasks and overwrite the keys if they're double.
        if let Some(platform) = platform {
            for target_metadata in self.target_specific_metadata(platform) {
                all_tasks.extend(target_metadata.tasks.keys());
            }
        }
        Vec::from_iter(all_tasks)
    }

    /// Returns a hashmap of the tasks that should run on the given platform.
    pub fn tasks(&self, platform: Option<Platform>) -> HashMap<&str, &Task> {
        let mut all_tasks = HashMap::default();

        // Gather non-target specific tasks
        all_tasks.extend(self.manifest.tasks.iter().map(|(k, v)| (k.as_str(), v)));

        // Gather platform-specific tasks and overwrite them if they're double.
        if let Some(platform) = platform {
            for target_metadata in self.target_specific_metadata(platform) {
                all_tasks.extend(target_metadata.tasks.iter().map(|(k, v)| (k.as_str(), v)));
            }
        }

        all_tasks
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

    /// Returns names of the tasks that depend on the given task.
    pub fn task_names_depending_on(&self, name: impl AsRef<str>) -> Vec<&str> {
        let mut tasks = self.tasks(Some(Platform::current()));
        let task = tasks.remove(name.as_ref());
        if task.is_some() {
            tasks
                .into_iter()
                .filter(|(_, c)| c.depends_on().contains(&name.as_ref().to_string()))
                .map(|(name, _)| name)
                .collect()
        } else {
            vec![]
        }
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
            ensure_toml_target_table(&mut self.doc, platform, "tasks")?
        } else {
            self.doc["tasks"]
                .or_insert(Item::Table(Table::new()))
                .as_table_mut()
                .ok_or_else(|| {
                    miette::miette!("target table in {} is malformed", consts::PROJECT_MANIFEST)
                })?
        };
        let depends_on = task.depends_on();

        for depends in depends_on {
            if !self.manifest.tasks.contains_key(depends) {
                miette::bail!(
                    "task '{}' for the depends on for '{}' does not exist",
                    depends,
                    name.as_ref(),
                );
            }
        }

        // Add the task to the table
        table.insert(name.as_ref(), task_as_toml(task.clone()));

        self.manifest.tasks.insert(name.as_ref().to_string(), task);

        self.save()?;

        Ok(())
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
            ensure_toml_target_table(&mut self.doc, platform, "tasks")?
        } else {
            let tasks_table = &mut self.doc["tasks"];
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

    pub fn load_or_else_discover(manifest_path: Option<&Path>) -> miette::Result<Self> {
        let project = match manifest_path {
            Some(path) => Project::load(path)?,
            None => Project::discover()?,
        };
        Ok(project)
    }

    pub fn reload(&mut self) -> miette::Result<()> {
        let project = Self::load(self.root().join(consts::PROJECT_MANIFEST).as_path())?;
        self.root = project.root;
        self.doc = project.doc;
        self.manifest = project.manifest;
        self.package_db = OnceCell::new();
        Ok(())
    }

    /// Loads a project manifest.
    pub fn from_manifest_str(root: &Path, contents: impl Into<String>) -> miette::Result<Self> {
        let contents = contents.into();
        let (manifest, doc) = match toml_edit::de::from_str::<ProjectManifest>(&contents)
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
            root: root.to_path_buf(),
            source: contents,
            doc,
            manifest,
            package_db: OnceCell::new(),
        })
    }

    /// Add a platform to the project
    pub fn add_platforms<'a>(
        &mut self,
        platforms: impl Iterator<Item = &'a Platform> + Clone,
    ) -> miette::Result<()> {
        // Add to platform table
        let platform_array = &mut self.doc["project"]["platforms"];
        let platform_array = platform_array
            .as_array_mut()
            .expect("platforms should be an array");

        for platform in platforms.clone() {
            platform_array.push(platform.to_string());
        }

        // Add to manifest
        self.manifest.project.platforms.value.extend(platforms);
        Ok(())
    }

    /// Returns the dependencies of the project.
    pub fn dependencies(
        &self,
        platform: Platform,
    ) -> miette::Result<IndexMap<PackageName, NamelessMatchSpec>> {
        // Get the base dependencies (defined in the `[dependencies]` section)
        let base_dependencies = self.manifest.dependencies.iter();

        // Get the platform specific dependencies in the order they were defined.
        let platform_specific = self
            .target_specific_metadata(platform)
            .flat_map(|target| target.dependencies.iter());

        // Combine the specs.
        //
        // Note that if a dependency was specified twice the platform specific one "wins".
        base_dependencies
            .chain(platform_specific)
            .map(|(name, spec)| PackageName::try_from(name).map(|name| (name, spec.clone())))
            .collect::<Result<_, _>>()
            .into_diagnostic()
    }

    /// Returns the build dependencies of the project.
    pub fn build_dependencies(
        &self,
        platform: Platform,
    ) -> miette::Result<IndexMap<PackageName, NamelessMatchSpec>> {
        // Get the base dependencies (defined in the `[build-dependencies]` section)
        let base_dependencies = self.manifest.build_dependencies.iter();

        // Get the platform specific dependencies in the order they were defined.
        let platform_specific = self
            .target_specific_metadata(platform)
            .flat_map(|target| target.build_dependencies.iter());

        // Combine the specs.
        //
        // Note that if a dependency was specified twice the platform specific one "wins".
        base_dependencies
            .chain(platform_specific)
            .flatten()
            .map(|(name, spec)| PackageName::try_from(name).map(|name| (name, spec.clone())))
            .collect::<Result<_, _>>()
            .into_diagnostic()
    }

    /// Returns the host dependencies of the project.
    pub fn host_dependencies(
        &self,
        platform: Platform,
    ) -> miette::Result<IndexMap<PackageName, NamelessMatchSpec>> {
        // Get the base dependencies (defined in the `[host-dependencies]` section)
        let base_dependencies = self.manifest.host_dependencies.iter();

        // Get the platform specific dependencies in the order they were defined.
        let platform_specific = self
            .target_specific_metadata(platform)
            .flat_map(|target| target.host_dependencies.iter());

        // Combine the specs.
        //
        // Note that if a dependency was specified twice the platform specific one "wins".
        base_dependencies
            .chain(platform_specific)
            .flatten()
            .map(|(name, spec)| PackageName::try_from(name).map(|name| (name, spec.clone())))
            .collect::<Result<_, _>>()
            .into_diagnostic()
    }

    /// Returns all dependencies of the project. These are the run, host, build dependency sets combined.
    pub fn all_dependencies(
        &self,
        platform: Platform,
    ) -> miette::Result<IndexMap<PackageName, NamelessMatchSpec>> {
        let mut dependencies = self.dependencies(platform)?;
        dependencies.extend(self.host_dependencies(platform)?);
        dependencies.extend(self.build_dependencies(platform)?);
        Ok(dependencies)
    }

    pub fn pypi_dependencies(&self) -> Option<&IndexMap<rip::PackageName, PyPiRequirement>> {
        self.manifest.pypi_dependencies.as_ref()
    }

    /// Returns the Python index URLs to use for this project.
    pub fn pypi_index_urls(&self) -> Vec<Url> {
        let index_url = normalize_index_url(Url::parse("https://pypi.org/simple/").unwrap());
        vec![index_url]
    }

    /// Returns the package database used for caching python metadata, wheels and more. See the
    /// documentation of [`rip::PackageDb`] for more information.
    pub fn pypi_package_db(&self) -> miette::Result<&rip::PackageDb> {
        Ok(self
            .package_db
            .get_or_try_init(|| {
                PackageDb::new(
                    default_client(),
                    &self.pypi_index_urls(),
                    rattler::default_cache_dir()
                        .map_err(|_| {
                            miette::miette!("could not determine default cache directory")
                        })?
                        .join("pypi/"),
                )
                .into_diagnostic()
                .map(Arc::new)
            })?
            .as_ref())
    }

    /// Returns all the targets specific metadata that apply with the given context.
    /// TODO: Add more context here?
    /// TODO: Should we return the selector too to provide better diagnostics later?
    pub fn target_specific_metadata(
        &self,
        platform: Platform,
    ) -> impl Iterator<Item = &'_ TargetMetadata> + '_ {
        self.manifest
            .target
            .iter()
            .filter_map(move |(selector, manifest)| match selector.as_ref() {
                TargetSelector::Platform(p) if p == &platform => Some(manifest),
                _ => None,
            })
    }

    /// Returns the name of the project
    pub fn name(&self) -> &str {
        &self.manifest.project.name
    }

    /// Returns the version of the project
    pub fn version(&self) -> &Option<Version> {
        &self.manifest.project.version
    }

    fn add_to_deps_table(
        deps_table: &mut Item,
        spec: &MatchSpec,
    ) -> miette::Result<(PackageName, NamelessMatchSpec)> {
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

    fn add_dep_to_target_table(
        &mut self,
        platform: Platform,
        dep_type: String,
        spec: &MatchSpec,
    ) -> miette::Result<(PackageName, NamelessMatchSpec)> {
        let target = self.doc["target"]
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                miette::miette!("target table in {} is malformed", consts::PROJECT_MANIFEST)
            })?;
        target.set_dotted(true);
        let platform_table = self.doc["target"][platform.as_str()]
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                miette::miette!(
                    "platform table in {} is malformed",
                    consts::PROJECT_MANIFEST
                )
            })?;
        platform_table.set_dotted(true);

        let dependencies = platform_table[dep_type.as_str()]
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                miette::miette!(
                    "platform table in {} is malformed",
                    consts::PROJECT_MANIFEST
                )
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
        dependencies.insert(name.as_source(), Item::Value(nameless.to_string().into()));

        Ok((name, nameless))
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
        self.manifest
            .target
            .entry(TargetSelector::Platform(platform).into())
            .or_insert(TargetMetadata::default())
            .dependencies
            .insert(name.as_source().into(), nameless);
        Ok(())
    }

    pub fn add_dependency(&mut self, spec: &MatchSpec, spec_type: SpecType) -> miette::Result<()> {
        // Find the dependencies table
        let deps = &mut self.doc[spec_type.name()];
        let (name, nameless) = Project::add_to_deps_table(deps, spec)?;

        self.manifest
            .create_or_get_dependencies(spec_type)
            .insert(name.as_source().into(), nameless);

        Ok(())
    }

    /// Removes a dependency from `pixi.toml` based on `SpecType`.
    pub fn remove_dependency(
        &mut self,
        dep: &PackageName,
        spec_type: &SpecType,
    ) -> miette::Result<(String, NamelessMatchSpec)> {
        if let Item::Table(ref mut t) = self.doc[spec_type.name()] {
            if t.contains_key(dep.as_normalized()) && t.remove(dep.as_normalized()).is_some() {
                self.save()?;
                return self
                    .manifest
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
        dep: &PackageName,
        spec_type: &SpecType,
        platform: &Platform,
    ) -> miette::Result<(String, NamelessMatchSpec)> {
        let table = get_toml_target_table(&mut self.doc, platform, spec_type.name())?;
        table.remove(dep.as_normalized());
        self.save()?;
        self.manifest
            .remove_target_dependency(dep.as_normalized(), spec_type, platform)
    }

    /// Returns the root directory of the project
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the pixi directory
    pub fn environment_dir(&self) -> PathBuf {
        self.root.join(consts::PIXI_DIR)
    }

    /// Returns the path to the manifest file.
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(consts::PROJECT_MANIFEST)
    }

    /// Returns the path to the lock file of the project
    pub fn lock_file_path(&self) -> PathBuf {
        self.root.join(consts::PROJECT_LOCK_FILE)
    }

    /// Save back changes
    pub fn save(&self) -> miette::Result<()> {
        fs::write(self.manifest_path(), self.doc.to_string())
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "unable to write changes to {}",
                    self.manifest_path().display()
                )
            })?;
        Ok(())
    }

    /// Returns the channels used by this project
    pub fn channels(&self) -> &[Channel] {
        &self.manifest.project.channels
    }

    /// Adds the specified channels to the project.
    pub fn add_channels(
        &mut self,
        channels: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> miette::Result<()> {
        let mut stored_channels = Vec::new();
        for channel in channels {
            self.manifest.project.channels.push(
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

    /// Replaces all the channels in the project with the specified channels.
    pub fn set_channels(
        &mut self,
        channels: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> miette::Result<()> {
        self.manifest.project.channels.clear();
        let mut stored_channels = Vec::new();
        for channel in channels {
            self.manifest.project.channels.push(
                Channel::from_str(channel.as_ref(), &ChannelConfig::default()).into_diagnostic()?,
            );
            stored_channels.push(channel.as_ref().to_owned());
        }

        let channels_array = self.channels_array_mut()?;
        channels_array.clear();
        for channel in stored_channels {
            channels_array.push(channel);
        }
        Ok(())
    }

    /// Returns a mutable reference to the channels array.
    fn channels_array_mut(&mut self) -> miette::Result<&mut Array> {
        let project = &mut self.doc["project"];
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

    /// Returns the platforms this project targets
    pub fn platforms(&self) -> &[Platform] {
        self.manifest.project.platforms.as_ref().as_slice()
    }

    /// Returns the all specified activation scripts that are used in the current platform.
    pub fn activation_scripts(&self, platform: Platform) -> miette::Result<Vec<PathBuf>> {
        let mut full_paths = Vec::new();
        let mut all_scripts = Vec::new();

        // Gather platform-specific activation scripts
        for target_metadata in self.target_specific_metadata(platform) {
            if let Some(activation) = &target_metadata.activation {
                if let Some(scripts) = &activation.scripts {
                    all_scripts.extend(scripts.clone());
                }
            }
        }

        // Gather the main activation scripts if there are no target scripts defined.
        if all_scripts.is_empty() {
            if let Some(activation) = &self.manifest.activation {
                if let Some(scripts) = &activation.scripts {
                    all_scripts.extend(scripts.clone());
                }
            }
        }

        // Check if scripts exist
        let mut missing_scripts = Vec::new();
        for script_name in &all_scripts {
            let script_path = self.root().join(script_name);
            if script_path.exists() {
                full_paths.push(script_path);
                tracing::debug!("Found activation script: {:?}", script_name);
            } else {
                missing_scripts.push(script_name);
            }
        }

        if !missing_scripts.is_empty() {
            tracing::warn!("can't find activation scripts: {:?}", missing_scripts);
        }

        Ok(full_paths)
    }

    /// Get the default task with the specified name or `None` if no such task exists.
    pub fn task_opt(&self, name: &str) -> Option<&Task> {
        self.manifest.tasks.get(name)
    }

    /// Get the system requirements defined under the `system-requirements` section of the project manifest.
    /// These get turned into virtual packages which are used in the solve.
    /// They will act as the description of a reference machine which is minimally needed for this package to be run.
    pub fn system_requirements(&self) -> Vec<VirtualPackage> {
        self.manifest.system_requirements.virtual_packages()
    }

    /// Get the system requirements defined under the `system-requirements` section of the project manifest.
    /// Excluding packages that are not relevant for the specified platform.
    pub fn system_requirements_for_platform(&self, platform: Platform) -> Vec<VirtualPackage> {
        // Filter system requirements based on the relevant packages for the current OS.
        self.manifest
            .system_requirements
            .virtual_packages()
            .iter()
            .filter(|requirement| {
                !non_relevant_virtual_packages_for_platform(requirement, platform)
            })
            .cloned()
            .collect()
    }
}

/// Iterates over the current directory and all its parent directories and returns the first
/// directory path that contains the [`consts::PROJECT_MANIFEST`].
pub fn find_project_root() -> Option<PathBuf> {
    let current_dir = env::current_dir().ok()?;
    std::iter::successors(Some(current_dir.as_path()), |prev| prev.parent())
        .find(|dir| dir.join(consts::PROJECT_MANIFEST).is_file())
        .map(Path::to_path_buf)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::manifest::SystemRequirements;
    use insta::assert_debug_snapshot;
    use rattler_conda_types::ChannelConfig;
    use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
    use std::str::FromStr;

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = []
        "#;

    #[test]
    fn test_main_project_config() {
        let file_content = r#"
            [project]
            name = "pixi"
            version = "0.0.2"
            channels = ["conda-forge"]
            platforms = ["linux-64", "win-64"]
        "#;

        let project = Project::from_manifest_str(Path::new(""), file_content.to_string()).unwrap();

        assert_eq!(project.name(), "pixi");
        assert_eq!(
            project.version().as_ref().unwrap(),
            &Version::from_str("0.0.2").unwrap()
        );
        assert_eq!(
            project.channels(),
            [Channel::from_name(
                "conda-forge",
                None,
                &ChannelConfig::default()
            )]
        );
        assert_eq!(
            project.platforms(),
            [
                Platform::from_str("linux-64").unwrap(),
                Platform::from_str("win-64").unwrap()
            ]
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
    fn test_system_requirements_edge_cases() {
        let file_contents = [
            r#"
        [system-requirements]
        libc = { version = "2.12" }
        "#,
            r#"
        [system-requirements]
        libc = "2.12"
        "#,
            r#"
        [system-requirements.libc]
        version = "2.12"
        "#,
            r#"
        [system-requirements.libc]
        version = "2.12"
        family = "glibc"
        "#,
        ];

        for file_content in file_contents {
            let file_content = format!("{PROJECT_BOILERPLATE}\n{file_content}");

            let project = Project::from_manifest_str(Path::new(""), &file_content).unwrap();

            let expected_result = vec![VirtualPackage::LibC(LibC {
                family: "glibc".to_string(),
                version: Version::from_str("2.12").unwrap(),
            })];

            let system_requirements = project.system_requirements();

            assert_eq!(system_requirements, expected_result);
        }
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
    fn test_dependency_sets() {
        let file_contents = r#"
        [dependencies]
        foo = "1.0"

        [host-dependencies]
        libc = "2.12"

        [build-dependencies]
        bar = "1.0"
        "#;

        let manifest = toml_edit::de::from_str::<ProjectManifest>(&format!(
            "{PROJECT_BOILERPLATE}\n{file_contents}"
        ))
        .unwrap();
        let project = Project::from_manifest(manifest);

        assert_debug_snapshot!(project.all_dependencies(Platform::Linux64).unwrap());
    }

    #[test]
    fn test_dependency_target_sets() {
        let file_contents = r#"
        [dependencies]
        foo = "1.0"

        [host-dependencies]
        libc = "2.12"

        [build-dependencies]
        bar = "1.0"

        [target.linux-64.build-dependencies]
        baz = "1.0"

        [target.linux-64.host-dependencies]
        banksy = "1.0"

        [target.linux-64.dependencies]
        wolflib = "1.0"
        "#;

        let manifest = toml_edit::de::from_str::<ProjectManifest>(&format!(
            "{PROJECT_BOILERPLATE}\n{file_contents}"
        ))
        .unwrap();
        let project = Project::from_manifest(manifest);

        assert_debug_snapshot!(project.all_dependencies(Platform::Linux64).unwrap());
    }
    #[test]
    fn test_activation_scripts() {
        // Using known files in the project so the test succeed including the file check.
        let file_contents = r#"
            [target.linux-64.activation]
            scripts = ["Cargo.toml"]

            [target.win-64.activation]
            scripts = ["Cargo.lock"]

            [activation]
            scripts = ["pixi.toml", "pixi.lock"]
            "#;

        let manifest = toml_edit::de::from_str::<ProjectManifest>(&format!(
            "{PROJECT_BOILERPLATE}\n{file_contents}"
        ))
        .unwrap();
        let project = Project::from_manifest(manifest);

        assert_debug_snapshot!(project.activation_scripts(Platform::Linux64).unwrap());
        assert_debug_snapshot!(project.activation_scripts(Platform::Win64).unwrap());
        assert_debug_snapshot!(project.activation_scripts(Platform::OsxArm64).unwrap());
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

        let mut project =
            Project::from_manifest_str(&PathBuf::from("/tmp/"), file_contents).unwrap();

        project
            .remove_target_dependency(
                &PackageName::try_from("baz").unwrap(),
                &SpecType::Build,
                &Platform::Linux64,
            )
            .unwrap();
        assert_debug_snapshot!(project.manifest);
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
            bar = "*"

            [target.linux-64.build-dependencies]
            baz = "*"
        "#;

        let mut project =
            Project::from_manifest_str(&PathBuf::from("/tmp/"), file_contents).unwrap();

        project
            .remove_dependency(&PackageName::try_from("fooz").unwrap(), &SpecType::Run)
            .unwrap();
        assert_debug_snapshot!(project.manifest);
    }

    #[test]
    fn test_target_specific_tasks() {
        // Using known files in the project so the test succeed including the file check.
        let file_contents = r#"
            [tasks]
            test = "test multi"

            [target.win-64.tasks]
            test = "test win"

            [target.linux-64.tasks]
            test = "test linux"
            "#;

        let manifest = toml_edit::de::from_str::<ProjectManifest>(&format!(
            "{PROJECT_BOILERPLATE}\n{file_contents}"
        ))
        .unwrap();
        let project = Project::from_manifest(manifest);

        assert_debug_snapshot!(project.tasks(Some(Platform::Osx64)));
        assert_debug_snapshot!(project.tasks(Some(Platform::Win64)));
        assert_debug_snapshot!(project.tasks(Some(Platform::Linux64)));
    }
}
