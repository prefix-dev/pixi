use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    io::Read,
    ops::Not,
    path::{Path, PathBuf},
    str::FromStr,
};

use ahash::HashSet;
use console::StyledObject;
use fs_err as fs;
use fs_err::tokio as tokio_fs;
use indexmap::{IndexMap, IndexSet};
use is_executable::IsExecutable;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pixi_config::pixi_home;
use pixi_consts::consts;
use pixi_manifest::PrioritizedChannel;
use pixi_utils::{executable_from_path, prefix::Executable};
use rattler::install::{Transaction, TransactionOperation};
use rattler_conda_types::{
    Channel, ChannelConfig, HasArtifactIdentificationRefs, NamedChannelOrUrl, PackageName,
    PrefixRecord, Version,
};
use url::Url;

use super::{
    EnvironmentName, ExposedName, Mapping,
    report::{self, EnvReport, EnvStatus, Item, Label, Marker, Row},
    trampoline::{GlobalExecutable, Trampoline},
};

/// Global binaries directory, default to `$HOME/.pixi/bin`
#[derive(Debug, Clone)]
pub struct BinDir(PathBuf);

impl BinDir {
    /// Create the binary executable directory from path
    #[cfg(test)]
    pub fn new(root: PathBuf) -> miette::Result<Self> {
        let path = root.join("bin");
        fs_err::create_dir_all(&path).into_diagnostic()?;
        Ok(Self(path))
    }

    /// Create the binary executable directory from environment variables
    pub async fn from_env() -> miette::Result<Self> {
        let bin_dir = pixi_home()
            .map(|path| path.join("bin"))
            .ok_or(miette::miette!(
                "Couldn't determine global binary executable directory"
            ))?;
        tokio_fs::create_dir_all(&bin_dir).await.into_diagnostic()?;
        Ok(Self(bin_dir))
    }

    /// Asynchronously retrieves all files in the binary executable directory.
    ///
    /// This function reads the directory specified by `self.0` and try to
    /// collect all file paths into a vector. It returns a `miette::Result`
    /// containing the vector of `GlobalExecutable`or an error if the
    /// directory can't be read.
    pub(crate) async fn executables(&self) -> miette::Result<Vec<GlobalExecutable>> {
        let mut files = Vec::new();
        let mut entries = tokio_fs::read_dir(&self.0).await.into_diagnostic()?;

        while let Some(entry) = entries.next_entry().await.into_diagnostic()? {
            let path = entry.path();
            if let Ok(trampoline) = Trampoline::try_from(&path).await {
                files.push(GlobalExecutable::Trampoline(trampoline));
            } else if path.is_file() && path.is_executable() && is_binary(&path)?.not() {
                // If the file is not a binary, it's a script
                files.push(GlobalExecutable::Script(path));
            }
        }

        Ok(files)
    }

    /// Returns the path to the binary directory
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Returns the path to the executable script for the given exposed name.
    ///
    /// This function constructs the path to the executable script by joining
    /// the `bin_dir` with the provided `exposed_name`. If the target
    /// platform is Windows, it sets the file extension to `.exe`.
    pub(crate) fn executable_trampoline_path(&self, exposed_name: &ExposedName) -> PathBuf {
        let exposed_name = if cfg!(windows) {
            // Not using `.set_extension()` because it will break the `.` in the name for
            // cases like `python3.9.1`
            format!("{exposed_name}.exe")
        } else {
            exposed_name.to_string()
        };
        self.path().join(exposed_name)
    }
}

/// Global environments directory, default to `$HOME/.pixi/envs`
#[derive(Debug, Clone)]
pub struct EnvRoot(PathBuf);

impl EnvRoot {
    /// Create the environment root directory
    #[cfg(test)]
    pub fn new(root: PathBuf) -> miette::Result<Self> {
        let path = root.join("envs");
        fs_err::create_dir_all(&path).into_diagnostic()?;
        Ok(Self(path))
    }

    /// Create the environment root directory from environment variables
    pub async fn from_env() -> miette::Result<Self> {
        let path = pixi_home()
            .map(|path| path.join("envs"))
            .ok_or_else(|| miette::miette!("Couldn't get home path"))?;
        tokio_fs::create_dir_all(&path).await.into_diagnostic()?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Get all directories in the env root
    pub(crate) async fn directories(&self) -> miette::Result<Vec<PathBuf>> {
        let mut directories = Vec::new();
        let mut entries = tokio_fs::read_dir(&self.path()).await.into_diagnostic()?;

        while let Some(entry) = entries.next_entry().await.into_diagnostic()? {
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
            }
        }

        Ok(directories)
    }
}

/// A global environment directory
pub struct EnvDir {
    pub(crate) path: PathBuf,
}

impl EnvDir {
    // Create EnvDir from path
    pub(crate) fn from_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Create a global environment directory based on passed global environment
    /// root
    pub(crate) async fn from_env_root(
        env_root: EnvRoot,
        environment_name: &EnvironmentName,
    ) -> miette::Result<Self> {
        let path = env_root.path().join(environment_name.as_str());
        tokio_fs::create_dir_all(&path).await.into_diagnostic()?;

        Ok(Self { path })
    }

    /// Construct the path to the env directory for the environment
    /// `environment_name`.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// Checks if a file is binary by reading the first 1024 bytes and checking for
/// null bytes.
pub(crate) fn is_binary(file_path: impl AsRef<Path>) -> miette::Result<bool> {
    let mut file = fs::File::open(file_path.as_ref()).into_diagnostic()?;
    let mut buffer = [0; 1024];
    let bytes_read = file.read(&mut buffer).into_diagnostic()?;

    Ok(buffer[..bytes_read].contains(&0))
}

/// Finds the package record from the `conda-meta` directory.
pub async fn find_package_records(conda_meta: &Path) -> miette::Result<Vec<PrefixRecord>> {
    let read_dir = tokio_fs::read_dir(conda_meta).await;
    let mut records = Vec::new();

    let mut read_dir = match read_dir {
        Ok(dir) => dir,
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => return Ok(records),
            _ => miette::bail!(
                "Failed to read conda-meta directory {}: {}",
                conda_meta.display(),
                e
            ),
        },
    };

    while let Some(entry) = read_dir.next_entry().await.into_diagnostic()? {
        let path = entry.path();
        // Check if the entry is a file and has a .json extension
        if path.is_file() && path.extension().and_then(OsStr::to_str) == Some("json") {
            let prefix_record = PrefixRecord::from_path(&path)
                .into_diagnostic()
                .wrap_err_with(|| format!("Couldn't parse json from {}", path.display()))?;

            records.push(prefix_record);
        }
    }

    Ok(records)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallChange {
    Installed(Version),
    Upgraded(Version, Version),
    TransitiveUpgraded(Version, Version),
    Reinstalled(Version, Version),
    Removed,
}

impl InstallChange {
    pub fn is_transitive(&self) -> bool {
        matches!(self, InstallChange::TransitiveUpgraded(_, _))
    }
    pub fn is_removed(&self) -> bool {
        matches!(self, InstallChange::Removed)
    }

    pub fn version_fancy_display(&self) -> Option<StyledObject<String>> {
        let version_style = console::Style::new().blue();
        let default_style = console::Style::new();

        match self {
            InstallChange::Installed(version) => Some(version_style.apply_to(version.to_string())),
            InstallChange::Upgraded(old, new) => Some(default_style.apply_to(format!(
                "{} -> {}",
                version_style.apply_to(old.to_string()),
                version_style.apply_to(new.to_string())
            ))),
            InstallChange::TransitiveUpgraded(old, new) => Some(default_style.apply_to(format!(
                "{} -> {}",
                version_style.apply_to(old.to_string()),
                version_style.apply_to(new.to_string())
            ))),
            InstallChange::Reinstalled(old, new) => Some(default_style.apply_to(format!(
                "{} -> {}",
                version_style.apply_to(old.to_string()),
                version_style.apply_to(new.to_string())
            ))),
            InstallChange::Removed => None,
        }
    }
}

/// Tracks changes made to the environment
/// after installing packages.
/// It also contain what packages were in environment before the update.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[must_use]
pub struct EnvironmentUpdate {
    package_changes: HashMap<PackageName, InstallChange>,
    current_packages: Vec<PackageName>,
}

impl EnvironmentUpdate {
    pub fn new(
        package_changes: HashMap<PackageName, InstallChange>,
        current_packages: Vec<PackageName>,
    ) -> Self {
        Self {
            package_changes,
            current_packages,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.package_changes.is_empty()
    }

    pub fn changes(&self) -> &HashMap<PackageName, InstallChange> {
        &self.package_changes
    }

    pub fn current_packages(&self) -> &Vec<PackageName> {
        &self.current_packages
    }

    pub fn add_removed_packages(&mut self, packages: Vec<PackageName>) {
        self.current_packages.extend(packages);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum StateChange {
    /// An executable was exposed, together with the executable it points at
    /// when that is known.
    AddedExposed(ExposedName, Option<String>),
    RemovedExposed(ExposedName),
    UpdatedExposed(ExposedName, Option<String>),
    AddedEnvironment,
    RemovedEnvironment,
    UpdatedEnvironment(EnvironmentUpdate),
    InstalledShortcut(String),
    UninstalledShortcut(String),
    AddedCompletion(String),
    RemovedCompletion(String),
}

#[must_use]
#[derive(Debug, Default)]
pub struct StateChanges {
    changes: HashMap<EnvironmentName, Vec<StateChange>>,
}

impl StateChanges {
    /// Creates a new `StateChanges` instance with a single environment name and
    /// an empty vector as its value.
    pub fn new_with_env(env_name: EnvironmentName) -> Self {
        Self {
            changes: HashMap::from([(env_name, Vec::new())]),
        }
    }

    /// Checks if there are any changes in the state.
    pub fn has_changed(&self) -> bool {
        !self.changes.values().all(Vec::is_empty)
    }

    pub fn insert_change(&mut self, env_name: &EnvironmentName, change: StateChange) {
        if let Some(entry) = self.changes.get_mut(env_name) {
            entry.push(change);
        } else {
            self.changes.insert(env_name.clone(), Vec::from([change]));
        }
    }

    #[cfg(test)]
    pub fn changes(self) -> HashMap<EnvironmentName, Vec<StateChange>> {
        self.changes
    }

    /// Remove changes that cancel each other out
    fn prune(&mut self) {
        self.changes = self
            .changes
            .iter()
            .map(|(env, changes_for_env)| {
                // Remove changes if the environment is removed afterwards
                let mut pruned_changes: Vec<StateChange> = Vec::new();
                for change in changes_for_env {
                    if let StateChange::RemovedEnvironment = change {
                        pruned_changes.clear();
                    }
                    pruned_changes.push(change.clone());
                }
                (env.clone(), pruned_changes)
            })
            .collect();
    }

    /// Turn the recorded changes into one report per environment, sorted by
    /// environment name so that the output doesn't depend on hash order.
    pub async fn into_reports(
        mut self,
        project: &super::Project,
    ) -> miette::Result<Vec<EnvReport>> {
        self.prune();

        let env_names = self
            .changes
            .keys()
            .cloned()
            .sorted_by(|left, right| left.as_str().cmp(right.as_str()))
            .collect_vec();

        let mut reports = Vec::with_capacity(env_names.len());
        for env_name in env_names {
            let changes = &self.changes[&env_name];
            reports.push(build_report(project, &env_name, changes).await?);
        }
        Ok(reports)
    }

    /// The reports for these changes, or nothing if they can't be built.
    /// Describing an operation is not part of performing it, so a failure here
    /// is a warning rather than something that affects the exit code.
    pub async fn reports_or_warn(self, project: &super::Project) -> Vec<EnvReport> {
        match self.into_reports(project).await {
            Ok(reports) => reports,
            Err(err) => {
                tracing::warn!("Couldn't describe what changed\n{err:?}");
                Vec::new()
            }
        }
    }

    /// Print a block for every environment these changes touched.
    pub async fn report(self, project: &super::Project) {
        for env_report in self.reports_or_warn(project).await {
            report::print(&env_report);
        }
    }
}

/// The executable an exposed name points at, spelled out only when it differs
/// from the name itself.
fn exposed_target(name: &ExposedName, executable: Option<&str>) -> Option<String> {
    executable
        .filter(|executable| *executable != name.to_string())
        .map(|executable| format!("-> {executable}"))
}

/// The item describing a single package change.
fn install_change_item(name: &str, change: &InstallChange) -> Item {
    let (marker, detail) = match change {
        InstallChange::Installed(version) => (Marker::Added, Some(version.to_string())),
        // A package can be rebuilt without its version moving, and `1.0 -> 1.0`
        // says less than the version on its own.
        InstallChange::Upgraded(old, new)
        | InstallChange::TransitiveUpgraded(old, new)
        | InstallChange::Reinstalled(old, new)
            if old == new =>
        {
            (Marker::Changed, Some(new.to_string()))
        }
        InstallChange::Upgraded(old, new)
        | InstallChange::TransitiveUpgraded(old, new)
        | InstallChange::Reinstalled(old, new) => {
            (Marker::Changed, Some(format!("{old} -> {new}")))
        }
        InstallChange::Removed => (Marker::Removed, None),
    };
    Item::package(marker, name, detail)
}

/// Whether the environment holds a single dependency named after the
/// environment itself, the case where its version goes into the header instead
/// of a `dependencies` row.
pub(crate) fn is_single_package_environment(
    project: &super::Project,
    env_name: &EnvironmentName,
) -> bool {
    project.environment(env_name).is_some_and(|environment| {
        environment.dependencies.specs.len() == 1
            && environment
                .dependencies
                .specs
                .keys()
                .next()
                .is_some_and(|name| name.as_normalized() == env_name.as_str())
    })
}

/// The installed version of the package an environment is named after.
///
/// Records are named `<package>-<version>-<build>.json`, so only the files that
/// could belong to this package are parsed rather than the whole prefix.
pub(crate) async fn installed_version(
    project: &super::Project,
    env_name: &EnvironmentName,
) -> miette::Result<Option<String>> {
    let conda_meta = project
        .env_root_path()
        .join(env_name.as_str())
        .join(consts::CONDA_META_DIR);

    let mut read_dir = match tokio_fs::read_dir(&conda_meta).await {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => miette::bail!(
            "Failed to read conda-meta directory {}: {}",
            conda_meta.display(),
            err
        ),
    };

    // Records written by conda or mamba keep the original case of the package
    // name in the file name, so the candidate test is case insensitive; the
    // parsed record still has to match by normalized name.
    let prefix = format!("{}-", env_name.as_str().to_lowercase());
    while let Some(entry) = read_dir.next_entry().await.into_diagnostic()? {
        let path = entry.path();
        if path.extension().and_then(OsStr::to_str) != Some("json") {
            continue;
        }
        let is_candidate = path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.to_lowercase().starts_with(&prefix));
        if !is_candidate {
            continue;
        }

        let record = PrefixRecord::from_path(&path)
            .into_diagnostic()
            .wrap_err_with(|| format!("Couldn't parse json from {}", path.display()))?;
        if record.repodata_record.package_record.name.as_normalized() == env_name.as_str() {
            return Ok(Some(
                record
                    .repodata_record
                    .package_record
                    .version
                    .version()
                    .to_string(),
            ));
        }
    }

    Ok(None)
}

fn push_row(rows: &mut Vec<Row>, label: Label, items: BTreeMap<String, Item>) {
    if !items.is_empty() {
        rows.push(Row::new(label, items.into_values().collect()));
    }
}

/// Build the report of a single environment out of the changes recorded for it.
async fn build_report(
    project: &super::Project,
    env_name: &EnvironmentName,
    changes: &[StateChange],
) -> miette::Result<EnvReport> {
    // Keyed by name so that an item is reported once with its net effect: a
    // `--force-reinstall` unexposes and re-exposes the same executable, and all
    // that matters is that it ended up exposed. Changes are walked in the order
    // they were recorded, so the last one wins.
    let mut dependencies: BTreeMap<String, Item> = BTreeMap::new();
    let mut exposed: BTreeMap<String, Item> = BTreeMap::new();
    let mut shortcuts: BTreeMap<String, Item> = BTreeMap::new();
    let mut completions: BTreeMap<String, Item> = BTreeMap::new();
    // The last environment-level change wins: `--force-reinstall` removes the
    // environment and creates it again, and that reads as an install.
    let mut environment_change = None;
    // Packages that aren't named in the manifest leave no row behind, but the
    // environment did change and has to say so.
    let mut transitive_change = false;

    for change in changes {
        match change {
            StateChange::AddedExposed(name, executable) => {
                exposed.insert(
                    name.to_string(),
                    Item::exposed(
                        Marker::Added,
                        name.to_string(),
                        exposed_target(name, executable.as_deref()),
                    ),
                );
            }
            StateChange::RemovedExposed(name) => {
                exposed.insert(
                    name.to_string(),
                    Item::exposed(Marker::Removed, name.to_string(), None),
                );
            }
            // Not reported: a rewritten trampoline doesn't change what the
            // `exposed` row says, and it happens as a side effect of almost any
            // change to the environment.
            StateChange::UpdatedExposed(_, _) => {}
            StateChange::AddedEnvironment => environment_change = Some(EnvStatus::Installed),
            StateChange::RemovedEnvironment => environment_change = Some(EnvStatus::Removed),
            StateChange::UpdatedEnvironment(update) => {
                for (package_name, install_change) in
                    update.changes().iter().sorted_by(|(left, _), (right, _)| {
                        left.as_normalized().cmp(right.as_normalized())
                    })
                {
                    // A package the manifest names is a dependency; everything
                    // else came along with it and is only counted.
                    if update.current_packages().contains(package_name) {
                        let name = package_name.as_normalized().to_string();
                        dependencies
                            .insert(name.clone(), install_change_item(&name, install_change));
                    } else {
                        transitive_change = true;
                    }
                }
            }
            StateChange::InstalledShortcut(name) => {
                shortcuts.insert(name.clone(), Item::plain(Marker::Added, name.clone()));
            }
            StateChange::UninstalledShortcut(name) => {
                shortcuts.insert(name.clone(), Item::plain(Marker::Removed, name.clone()));
            }
            StateChange::AddedCompletion(name) => {
                completions.insert(name.clone(), Item::plain(Marker::Added, name.clone()));
            }
            StateChange::RemovedCompletion(name) => {
                completions.insert(name.clone(), Item::plain(Marker::Removed, name.clone()));
            }
        }
    }

    // An environment that holds a single dependency named after itself says
    // everything about it in the header, so a row would repeat the name
    // standing right above it.
    let single_package = is_single_package_environment(project, env_name);
    let own_dependency = single_package
        .then(|| dependencies.remove(env_name.as_str()))
        .flatten();

    let mut rows = Vec::new();
    push_row(&mut rows, Label::Dependencies, dependencies);
    push_row(&mut rows, Label::Exposed, exposed);
    push_row(&mut rows, Label::Shortcuts, shortcuts);
    push_row(&mut rows, Label::Completions, completions);

    let status = match environment_change {
        Some(status) => status,
        // The dependency that was lifted into the header and the packages that
        // came along with it still count as changes, even though they left no
        // row behind.
        None if rows.is_empty() && own_dependency.is_none() && !transitive_change => {
            EnvStatus::Unchanged
        }
        None => EnvStatus::Updated,
    };

    let version = if status == EnvStatus::Removed || !single_package {
        None
    } else if let Some(dependency) = &own_dependency {
        // The change rather than the resulting version alone, so an upgrade
        // reads `0.26.1 -> 0.27.0` in the header.
        dependency.detail.clone()
    } else {
        installed_version(project, env_name).await?
    };

    Ok(EnvReport::new(env_name.as_str(), version, Some(status)).with_rows(rows))
}

impl std::ops::BitOrAssign for StateChanges {
    fn bitor_assign(&mut self, rhs: Self) {
        for (env_name, changes_for_env) in rhs.changes {
            self.changes
                .entry(env_name)
                .or_default()
                .extend(changes_for_env);
        }
    }
}

/// converts a channel url string to a PrioritizedChannel
pub(crate) fn channel_url_to_prioritized_channel(
    channel: &str,
    channel_config: &ChannelConfig,
) -> miette::Result<PrioritizedChannel> {
    // If channel url contains channel config alias as a substring, don't use it as
    // a URL
    if channel.contains(channel_config.channel_alias.as_str()) {
        // Create channel from URL for parsing
        let channel = Channel::from_url(Url::from_str(channel).expect("channel should be url"));
        // If it has a name return as named channel
        if let Some(name) = channel.name {
            // If the channel has a name, use it as the channel
            return Ok(NamedChannelOrUrl::from_str(&name).into_diagnostic()?.into());
        }
    }
    // If channel doesn't contain the alias or has no name, use it as a URL
    Ok(NamedChannelOrUrl::from_str(channel)
        .into_diagnostic()?
        .into())
}

/// Determines which shortcuts need to be installed or removed by comparing the
/// requested shortcuts with the installed package records.
///
/// This function filters the provided `prefix_records` to find those that
/// contain menuinst JSON files. It then compares these records with the
/// requested `shortcuts` to determine which records need to be installed and
/// which need to be uninstalled.
pub(crate) fn shortcuts_sync_status(
    shortcuts: IndexSet<PackageName>,
    prefix_records: Vec<PrefixRecord>,
    prefix_root: &Path,
) -> miette::Result<(Vec<PrefixRecord>, Vec<PrefixRecord>)> {
    let mut remaining_shortcuts = shortcuts;
    let mut records_to_install = Vec::new();
    let mut records_to_uninstall = Vec::new();

    let records_with_menuinst = prefix_records
        .into_iter()
        .filter(|record| contains_menuinst_document(record, prefix_root));

    for record in records_with_menuinst {
        let has_installed_system_menus = record.installed_system_menus.is_empty().not();
        if remaining_shortcuts
            .swap_take(&record.repodata_record.package_record.name)
            .is_some()
        {
            if !has_installed_system_menus {
                // The package record isn't installed, but it is requested
                records_to_install.push(record);
            }
        } else if has_installed_system_menus {
            // The package record is installed, but not requested
            records_to_uninstall.push(record);
        }
    }

    if remaining_shortcuts.is_empty().not() {
        miette::bail!(
            "the following shortcuts are requested but not available: {}",
            remaining_shortcuts
                .iter()
                .map(|n| n.as_normalized())
                .join(", ")
        );
    }
    Ok((records_to_install, records_to_uninstall))
}

pub fn contains_menuinst_document(prefix_record: &PrefixRecord, prefix_root: &Path) -> bool {
    for file in &prefix_record.files {
        if file.extension().is_some_and(|ext| ext == "json")
            && let Some(parent) = file.parent()
            && parent.file_name().is_some_and(|f| f == "Menu")
            && let Ok(content) = fs::read_to_string(prefix_root.join(file))
        {
            if let Err(err) =
                serde_json::from_str::<rattler_menuinst::schema::MenuInstSchema>(&content)
            {
                tracing::warn!(
                    "{} contains shortcuts, but they couldn't be parsed: {}",
                    console::style(
                        prefix_record
                            .repodata_record
                            .package_record
                            .name
                            .as_normalized()
                    )
                    .green(),
                    err
                )
            } else {
                return true;
            }
        }
    }
    false
}

/// Figures out what the status is of the exposed binaries of the environment.
///
/// Returns a tuple of the exposed binaries to remove and the exposed binaries
/// to add.
pub(crate) async fn expose_scripts_sync_status(
    bin_dir: &BinDir,
    env_dir: &EnvDir,
    mappings: &IndexSet<Mapping>,
) -> miette::Result<(Vec<GlobalExecutable>, IndexSet<ExposedName>)> {
    // Get all paths to the binaries from trampolines or scripts in the bin
    // directory.
    let locally_exposed = bin_dir.executables().await?;
    let executable_paths = futures::future::join_all(locally_exposed.iter().map(|global_bin| {
        let global_bin = global_bin.clone();
        let path = global_bin.path().clone();
        async move {
            global_bin
                .executable()
                .await
                .ok()
                .map(|exec| (path, exec, global_bin))
        }
    }))
    .await
    .into_iter()
    .flatten()
    .collect_vec();

    // Filter out all binaries that are related to the environment
    let related = executable_paths
        .into_iter()
        .filter(|(_, exec, _)| exec.starts_with(env_dir.path()))
        .collect_vec();

    fn match_mapping(mapping: &Mapping, exposed: &Path, executable: &Path) -> bool {
        executable_from_path(exposed) == mapping.exposed_name().to_string()
            && executable_from_path(executable) == mapping.executable_name()
    }

    // Get all related expose scripts not required by the environment manifest
    let to_remove = related
        .iter()
        .filter_map(|(exposed, executable, bin_type)| {
            if mappings
                .iter()
                .any(|mapping| match_mapping(mapping, exposed, executable))
                && bin_type.is_trampoline()
            {
                None
            } else {
                Some(bin_type)
            }
        })
        .cloned()
        .collect_vec();

    // Get all required exposed binaries that are not yet exposed
    let to_add = mappings
        .iter()
        .filter_map(|mapping| {
            if related.iter().any(|(exposed, executable, bin)| {
                match_mapping(mapping, exposed, executable) && bin.is_trampoline()
            }) {
                None
            } else {
                Some(mapping.exposed_name().clone())
            }
        })
        .collect::<IndexSet<ExposedName>>();

    Ok((to_remove, to_add))
}

/// Check if all binaries were exposed, or if the user selected a subset of
/// them.
pub fn check_all_exposed(
    env_binaries: &IndexMap<PackageName, Vec<Executable>>,
    exposed_mapping_binaries: &IndexSet<Mapping>,
) -> bool {
    let mut env_binaries_names_iter = env_binaries
        .values()
        .flatten()
        .map(|executable| executable.name.clone());

    let exposed_binaries_names: HashSet<&str> = exposed_mapping_binaries
        .iter()
        .map(|mapping| mapping.executable_name())
        .collect();

    env_binaries_names_iter.all(|name| exposed_binaries_names.contains(&name.as_str()))
}

pub(crate) fn get_install_changes<
    Old: HasArtifactIdentificationRefs,
    New: HasArtifactIdentificationRefs,
>(
    install_transaction: Transaction<Old, New>,
) -> HashMap<PackageName, InstallChange> {
    install_transaction
        .operations
        .into_iter()
        .map(|transaction| match transaction {
            TransactionOperation::Install(package) => {
                let pkg_name = package.name();

                (
                    pkg_name.clone(),
                    InstallChange::Installed(package.version().version().clone()),
                )
            }
            TransactionOperation::Change { old, new } => {
                let old_pkg_version = old.version();
                let new_pkg_version = new.version();

                let pkg_name = new.name();

                let same_base_version = old_pkg_version == new_pkg_version;

                let change = if same_base_version {
                    InstallChange::TransitiveUpgraded(
                        old_pkg_version.version().clone(),
                        new_pkg_version.version().clone(),
                    )
                } else {
                    InstallChange::Upgraded(
                        old_pkg_version.version().clone(),
                        new_pkg_version.version().clone(),
                    )
                };

                (pkg_name.clone(), change)
            }
            TransactionOperation::Reinstall { old, new } => {
                let pkg_name = new.name();
                (
                    pkg_name.clone(),
                    InstallChange::Reinstalled(
                        old.version().version().clone(),
                        new.version().version().clone(),
                    ),
                )
            }
            TransactionOperation::Remove(package) => {
                let pkg_name = package.name();
                (pkg_name.clone(), InstallChange::Removed)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rstest::rstest;
    use tempfile::tempdir;

    use super::*;
    use crate::trampoline::Configuration;

    #[tokio::test]
    async fn test_create() {
        // Create a temporary directory
        let temp_dir = tempdir().unwrap();

        // Set the env root to the temporary directory
        let env_root = EnvRoot::new(temp_dir.path().to_owned()).unwrap();

        // Define a test environment name
        let environment_name = &EnvironmentName::from_str("test-env").unwrap();

        // Create a new binary env dir
        let bin_env_dir = EnvDir::from_env_root(env_root, environment_name)
            .await
            .unwrap();

        // Verify that the directory was created
        assert!(bin_env_dir.path().exists());
        assert!(bin_env_dir.path().is_dir());
    }

    #[tokio::test]
    async fn test_find_package_record() {
        // Get meta file from test data folder relative to the current file
        let dummy_conda_meta_path = PathBuf::from(env!("CARGO_WORKSPACE_DIR"))
            .join("crates")
            .join("pixi_global")
            .join("src")
            .join("test_data")
            .join("conda-meta");
        // Find the package record
        let records = find_package_records(&dummy_conda_meta_path).await.unwrap();

        // Verify that the package record was found
        assert!(
            records
                .iter()
                .any(|rec| rec.repodata_record.package_record.name.as_normalized() == "python")
        );
    }

    #[test]
    fn test_channel_url_to_prioritized_channel() {
        let channel_config = ChannelConfig {
            channel_alias: Url::from_str("https://conda.anaconda.org").unwrap(),
            root_dir: PathBuf::from("/tmp"),
        };
        // Same host as alias
        let channel = "https://conda.anaconda.org/conda-forge";
        let prioritized_channel =
            channel_url_to_prioritized_channel(channel, &channel_config).unwrap();
        assert_eq!(
            PrioritizedChannel::from(NamedChannelOrUrl::from_str("conda-forge").unwrap()),
            prioritized_channel
        );

        // Different host
        let channel = "https://prefix.dev/conda-forge";
        let prioritized_channel =
            channel_url_to_prioritized_channel(channel, &channel_config).unwrap();
        assert_eq!(
            PrioritizedChannel::from(
                NamedChannelOrUrl::from_str("https://prefix.dev/conda-forge").unwrap()
            ),
            prioritized_channel
        );

        // File URL
        let channel = "file:///C:/Users/user/channel/output";
        let prioritized_channel =
            channel_url_to_prioritized_channel(channel, &channel_config).unwrap();
        assert_eq!(
            PrioritizedChannel::from(
                NamedChannelOrUrl::from_str("file:///C:/Users/user/channel/output").unwrap()
            ),
            prioritized_channel
        );
    }

    #[rstest]
    #[case("python3.9.1")]
    #[case("python3.9")]
    #[case("python3")]
    #[case("python")]
    fn test_executable_script_path(#[case] exposed_name: &str) {
        let path = PathBuf::from("/home/user/.pixi/bin");
        let bin_dir = BinDir(path.clone());
        let exposed_name = ExposedName::from_str(exposed_name).unwrap();
        let executable_script_path = bin_dir.executable_trampoline_path(&exposed_name);

        if cfg!(windows) {
            let expected = format!("{exposed_name}.exe");
            assert_eq!(executable_script_path, path.join(expected));
        } else {
            assert_eq!(executable_script_path, path.join(exposed_name.to_string()));
        }
    }

    #[tokio::test]
    async fn test_get_expose_scripts_sync_status_for_legacy_scripts() {
        let tmp_home_dir = tempfile::tempdir().unwrap();
        let tmp_home_dir_path = tmp_home_dir.path().to_path_buf();
        let env_root = EnvRoot::new(tmp_home_dir_path.clone()).unwrap();
        let env_name = EnvironmentName::from_str("test").unwrap();
        let env_dir = EnvDir::from_env_root(env_root, &env_name).await.unwrap();
        let bin_dir = BinDir::new(tmp_home_dir_path.clone()).unwrap();

        // Test empty
        let exposed = IndexSet::new();
        let (to_remove, to_add) = expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
            .await
            .unwrap();
        assert!(to_remove.is_empty());
        assert!(to_add.is_empty());

        // Test with exposed
        let mut exposed = IndexSet::new();
        exposed.insert(Mapping::new(
            ExposedName::from_str("test").unwrap(),
            "test".to_string(),
        ));
        exposed.insert(Mapping::new(
            ExposedName::from_str("nested_test").unwrap(),
            Path::new("other_dir")
                .join("nested_test")
                .to_str()
                .unwrap()
                .to_string(),
        ));
        let (to_remove, to_add) = expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
            .await
            .unwrap();
        assert!(to_remove.is_empty());
        assert_eq!(to_add.len(), 2);

        // Add a legacy script to the bin directory
        // even if it should be exposed and it's pointing to correct executable
        // it is an old script
        // we need to remove it and replace with trampoline
        let script_names = ["test", "nested_test"];

        #[cfg(windows)]
        {
            for script_name in script_names {
                let script_path = bin_dir.path().join(format!("{script_name}.bat"));
                let script = format!(
                    r#"
            @"{}" %*
            "#,
                    env_dir
                        .path()
                        .join("bin")
                        .join(format!("{script_name}.exe"))
                        .to_string_lossy()
                );
                tokio_fs::write(&script_path, script).await.unwrap();
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            for script_name in script_names {
                let script_path = bin_dir.path().join(script_name);
                let script = format!(
                    r#"#!/bin/sh
            "{}" "$@"
            "#,
                    env_dir
                        .path()
                        .join("bin")
                        .join(script_name)
                        .to_string_lossy()
                );
                tokio_fs::write(&script_path, script).await.unwrap();
                // Set the file permissions to make it executable
                let metadata = tokio_fs::metadata(&script_path).await.unwrap();
                let mut permissions = metadata.permissions();
                permissions.set_mode(0o755); // rwxr-xr-x
                tokio_fs::set_permissions(&script_path, permissions)
                    .await
                    .unwrap();
            }
        };

        // Test to_remove and to_add to see if the legacy scripts are removed and
        // trampolines are added
        let (to_remove, to_add) = expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
            .await
            .unwrap();
        assert!(to_remove.iter().all(|bin| !bin.is_trampoline()));
        assert_eq!(to_remove.len(), 2);
        assert_eq!(to_add.len(), 2);

        // Test to_remove when nothing should be exposed
        // it should remove all the legacy scripts and add nothing
        let (to_remove, to_add) = expose_scripts_sync_status(&bin_dir, &env_dir, &IndexSet::new())
            .await
            .unwrap();

        assert!(to_remove.iter().all(|bin| !bin.is_trampoline()));
        assert_eq!(to_remove.len(), 2);
        assert!(to_add.is_empty());
    }

    #[tokio::test]
    async fn test_get_expose_scripts_sync_status_for_trampolines() {
        let tmp_home_dir = tempfile::tempdir().unwrap();
        let tmp_home_dir_path = tmp_home_dir.path().to_path_buf();
        let env_root = EnvRoot::new(tmp_home_dir_path.clone()).unwrap();
        let env_name = EnvironmentName::from_str("test").unwrap();
        let env_dir = EnvDir::from_env_root(env_root, &env_name).await.unwrap();
        let bin_dir = BinDir::new(tmp_home_dir_path.clone()).unwrap();

        // Test empty
        let exposed = IndexSet::new();
        let (to_remove, to_add) = expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
            .await
            .unwrap();
        assert!(to_remove.is_empty());
        assert!(to_add.is_empty());

        // Test with exposed
        let mut exposed = IndexSet::new();
        exposed.insert(Mapping::new(
            ExposedName::from_str("test").unwrap(),
            "test".to_string(),
        ));

        let (to_remove, to_add) = expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
            .await
            .unwrap();
        assert!(to_remove.is_empty());
        assert_eq!(to_add.len(), 1);

        // add a trampoline
        let original_exe = if cfg!(windows) {
            env_dir.path().join("bin/test.exe")
        } else {
            env_dir.path().join("bin/test")
        };

        let manifest = Configuration::new(original_exe, String::new(), HashMap::new());
        let trampoline = Trampoline::new(
            ExposedName::from_str("test").unwrap(),
            bin_dir.path().to_path_buf(),
            manifest,
        );

        trampoline.save().await.unwrap();

        let (to_remove, to_add) = expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
            .await
            .unwrap();

        assert!(to_remove.is_empty());
        assert!(to_add.is_empty());

        // Test to_remove when nothing should be exposed
        let (mut to_remove, to_add) =
            expose_scripts_sync_status(&bin_dir, &env_dir, &IndexSet::new())
                .await
                .unwrap();
        assert_eq!(to_remove.len(), 1);

        assert_eq!(to_remove.pop().unwrap().exposed_name().to_string(), "test");
        assert!(to_add.is_empty());
    }
}
