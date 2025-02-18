use super::trampoline::{GlobalExecutable, Trampoline};
use super::{EnvironmentName, ExposedName, Mapping};
use crate::prefix::Executable;

use ahash::HashSet;
use console::StyledObject;
use fancy_display::FancyDisplay;
use fs_err as fs;
use fs_err::tokio as tokio_fs;
use indexmap::{IndexMap, IndexSet};
use is_executable::IsExecutable;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pixi_config::pixi_home;
use pixi_manifest::PrioritizedChannel;
use pixi_utils::executable_from_path;
use rattler::install::{Transaction, TransactionOperation};
use rattler_conda_types::{
    Channel, ChannelConfig, NamedChannelOrUrl, PackageName, PackageRecord, PrefixRecord,
    RepoDataRecord, Version,
};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::iter::Peekable;
use std::ops::Not;
use std::str::FromStr;
use std::{
    io::Read,
    path::{Path, PathBuf},
};
use url::Url;

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
    /// This function reads the directory specified by `self.0` and try to collect all
    /// file paths into a vector. It returns a `miette::Result` containing the
    /// vector of `GlobalExecutable`or an error if the directory can't be read.
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
    /// This function constructs the path to the executable script by joining the
    /// `bin_dir` with the provided `exposed_name`. If the target platform is
    /// Windows, it sets the file extension to `.exe`.
    pub(crate) fn executable_trampoline_path(&self, exposed_name: &ExposedName) -> PathBuf {
        // Add .bat to the windows executable
        let exposed_name = if cfg!(windows) {
            // Not using `.set_extension()` because it will break the `.` in the name for cases like `python3.9.1`
            format!("{}.exe", exposed_name)
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
    pub(crate) async fn from_env() -> miette::Result<Self> {
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
pub(crate) struct EnvDir {
    pub(crate) path: PathBuf,
}

impl EnvDir {
    // Create EnvDir from path
    pub(crate) fn from_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Create a global environment directory based on passed global environment root
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

/// Checks if a file is binary by reading the first 1024 bytes and checking for null bytes.
pub(crate) fn is_binary(file_path: impl AsRef<Path>) -> miette::Result<bool> {
    let mut file = fs::File::open(file_path.as_ref()).into_diagnostic()?;
    let mut buffer = [0; 1024];
    let bytes_read = file.read(&mut buffer).into_diagnostic()?;

    Ok(buffer[..bytes_read].contains(&0))
}

/// Finds the package record from the `conda-meta` directory.
pub(crate) async fn find_package_records(conda_meta: &Path) -> miette::Result<Vec<PrefixRecord>> {
    let mut read_dir = tokio_fs::read_dir(conda_meta).await.into_diagnostic()?;
    let mut records = Vec::new();

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

    if records.is_empty() {
        miette::bail!("No package records found in {}", conda_meta.display());
    }

    Ok(records)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotChangedReason {
    AlreadyInstalled,
}

impl std::fmt::Display for NotChangedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotChangedReason::AlreadyInstalled => {
                write!(f, "{}", NotChangedReason::AlreadyInstalled.as_str())
            }
        }
    }
}

impl NotChangedReason {
    /// Returns the name of the environment.
    pub fn as_str(&self) -> &str {
        match self {
            NotChangedReason::AlreadyInstalled => "already installed",
        }
    }
}

impl FancyDisplay for NotChangedReason {
    fn fancy_display(&self) -> StyledObject<&str> {
        console::style(self.as_str()).cyan()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EnvState {
    Installed,
    NotChanged(NotChangedReason),
}

impl EnvState {
    pub fn as_str(&self) -> &str {
        match self {
            EnvState::Installed => "installed",
            EnvState::NotChanged(reason) => reason.as_str(),
        }
    }
}

impl FancyDisplay for EnvState {
    fn fancy_display(&self) -> StyledObject<&str> {
        match self {
            EnvState::Installed => console::style(self.as_str()).green(),
            EnvState::NotChanged(ref reason) => reason.fancy_display(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct EnvChanges {
    pub changes: HashMap<EnvironmentName, EnvState>,
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
pub(crate) struct EnvironmentUpdate {
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
pub(crate) enum StateChange {
    AddedExposed(ExposedName),
    RemovedExposed(ExposedName),
    UpdatedExposed(ExposedName),
    AddedPackage(PackageRecord),
    AddedEnvironment,
    RemovedEnvironment,
    UpdatedEnvironment(EnvironmentUpdate),
}

#[must_use]
#[derive(Debug, Default)]
pub(crate) struct StateChanges {
    changes: HashMap<EnvironmentName, Vec<StateChange>>,
}

impl StateChanges {
    /// Creates a new `StateChanges` instance with a single environment name and an empty vector as its value.
    pub(crate) fn new_with_env(env_name: EnvironmentName) -> Self {
        Self {
            changes: HashMap::from([(env_name, Vec::new())]),
        }
    }

    /// Checks if there are any changes in the state.
    pub(crate) fn has_changed(&self) -> bool {
        !self.changes.values().all(Vec::is_empty)
    }

    pub(crate) fn insert_change(&mut self, env_name: &EnvironmentName, change: StateChange) {
        if let Some(entry) = self.changes.get_mut(env_name) {
            entry.push(change);
        } else {
            self.changes.insert(env_name.clone(), Vec::from([change]));
        }
    }

    pub(crate) fn push_changes(
        &mut self,
        env_name: &EnvironmentName,
        changes: impl IntoIterator<Item = StateChange>,
    ) {
        if let Some(entry) = self.changes.get_mut(env_name) {
            entry.extend(changes);
        } else {
            self.changes
                .insert(env_name.clone(), changes.into_iter().collect());
        }
    }

    #[cfg(test)]
    pub fn changes(self) -> HashMap<EnvironmentName, Vec<StateChange>> {
        self.changes
    }

    fn accumulate_changes<F, T>(
        iter: &mut Peekable<std::slice::Iter<StateChange>>,
        filter_fn: F,
        init_value: Option<T>,
    ) -> Vec<T>
    where
        F: Fn(Option<&StateChange>) -> Option<T>,
    {
        let mut changes = init_value.into_iter().collect::<Vec<T>>();

        while let Some(next) = filter_fn(iter.peek().cloned()) {
            changes.push(next);
            iter.next();
        }

        changes
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
            .collect()
    }

    pub(crate) fn report(mut self) {
        self.prune();

        for (env_name, changes_for_env) in self.changes {
            // If there are no changes for the environment, skip it
            if changes_for_env.is_empty() {
                continue;
            }

            let mut iter = changes_for_env.iter().peekable();

            while let Some(change) = iter.next() {
                match change {
                    StateChange::AddedExposed(first_name) => {
                        let mut exposed_names = StateChanges::accumulate_changes(
                            &mut iter,
                            |next| match next {
                                Some(StateChange::AddedExposed(name)) => Some(name.clone()),
                                _ => None,
                            },
                            Some(first_name.clone()),
                        );

                        exposed_names.sort();

                        if exposed_names.len() == 1 {
                            eprintln!(
                                "{}Exposed executable {} from environment {}.",
                                console::style(console::Emoji("✔ ", "")).green(),
                                exposed_names[0].fancy_display(),
                                env_name.fancy_display()
                            );
                        } else {
                            eprintln!(
                                "{}Exposed executables from environment {}:",
                                console::style(console::Emoji("✔ ", "")).green(),
                                env_name.fancy_display()
                            );
                            for exposed_name in exposed_names {
                                eprintln!("   - {}", exposed_name.fancy_display());
                            }
                        }
                    }
                    StateChange::RemovedExposed(removed) => {
                        let mut exposed_names = StateChanges::accumulate_changes(
                            &mut iter,
                            |next| match next {
                                Some(StateChange::RemovedExposed(name)) => Some(name.clone()),
                                _ => None,
                            },
                            Some(removed.clone()),
                        );
                        exposed_names.sort();
                        if exposed_names.len() == 1 {
                            eprintln!(
                                "{}Removed exposed executable {} from environment {}.",
                                console::style(console::Emoji("✔ ", "")).green(),
                                exposed_names[0].fancy_display(),
                                env_name.fancy_display()
                            );
                        } else {
                            eprintln!(
                                "{}Removed exposed executables from environment {}:",
                                console::style(console::Emoji("✔ ", "")).green(),
                                env_name.fancy_display()
                            );
                            for exposed_name in exposed_names {
                                eprintln!("   - {}", exposed_name.fancy_display());
                            }
                        }
                    }
                    StateChange::UpdatedExposed(exposed) => {
                        let mut exposed_names = StateChanges::accumulate_changes(
                            &mut iter,
                            |next| match next {
                                Some(StateChange::UpdatedExposed(name)) => Some(name.clone()),
                                _ => None,
                            },
                            Some(exposed.clone()),
                        );
                        exposed_names.sort();
                        if exposed_names.len() == 1 {
                            eprintln!(
                                "{}Updated executable {} of environment {}.",
                                console::style(console::Emoji("✔ ", "")).green(),
                                exposed_names[0].fancy_display(),
                                env_name.fancy_display()
                            );
                        } else {
                            eprintln!(
                                "{}Updated executables of environment {}:",
                                console::style(console::Emoji("✔ ", "")).green(),
                                env_name.fancy_display()
                            );
                            for exposed_name in exposed_names {
                                eprintln!("   - {}", exposed_name.fancy_display());
                            }
                        }
                    }
                    StateChange::AddedPackage(pkg) => {
                        let mut added_pkgs = StateChanges::accumulate_changes(
                            &mut iter,
                            |next| match next {
                                Some(StateChange::AddedPackage(name)) => Some(name.clone()),
                                _ => None,
                            },
                            Some(pkg.clone()),
                        );

                        added_pkgs.sort_by(|pkg1, pkg2| pkg1.name.cmp(&pkg2.name));

                        if added_pkgs.len() == 1 {
                            eprintln!(
                                "{}Added package {}={} to environment {}.",
                                console::style(console::Emoji("✔ ", "")).green(),
                                console::style(pkg.name.as_normalized()).green(),
                                console::style(&pkg.version).blue(),
                                env_name.fancy_display()
                            );
                        } else {
                            eprintln!(
                                "{}Added packages of environment {}:",
                                console::style(console::Emoji("✔ ", "")).green(),
                                env_name.fancy_display()
                            );
                            for pkg in added_pkgs {
                                eprintln!(
                                    "   - {}={}",
                                    console::style(pkg.name.as_normalized()).green(),
                                    console::style(&pkg.version).blue(),
                                );
                            }
                        }
                    }
                    StateChange::AddedEnvironment => {
                        eprintln!(
                            "{}Added environment {}.",
                            console::style(console::Emoji("✔ ", "")).green(),
                            env_name.fancy_display()
                        );
                    }
                    StateChange::RemovedEnvironment => {
                        eprintln!(
                            "{}Removed environment {}.",
                            console::style(console::Emoji("✔ ", "")).green(),
                            env_name.fancy_display()
                        );
                    }
                    StateChange::UpdatedEnvironment(update_change) => {
                        StateChanges::report_update_changes(&env_name, update_change);
                    }
                }
            }
        }
    }

    pub(crate) fn report_update_changes(
        env_name: &EnvironmentName,
        environment_update: &EnvironmentUpdate,
    ) {
        // Check if there are any changes
        if environment_update.is_empty() {
            eprintln!(
                "{}Environment {} was already up-to-date.",
                console::style(console::Emoji("✔ ", "")).green(),
                env_name.fancy_display(),
            );
            return;
        }

        // Separate top-level and transitive changes
        let mut top_level_changes: Vec<(&PackageName, &InstallChange)> = Vec::new();
        let mut transitive_changes = Vec::new();

        let env_dependencies = environment_update.current_packages();

        for (package_name, change) in environment_update.changes() {
            if env_dependencies.contains(package_name) && !change.is_transitive() {
                top_level_changes.push((package_name, change));
            } else if change.is_transitive() {
                transitive_changes.push((package_name, change));
            }
        }

        top_level_changes.sort_by(|(name1, _), (name2, _)| name1.cmp(name2));

        let was_removed = top_level_changes
            .iter()
            .find_map(|(_, change)| change.is_removed().then_some(|| true))
            .is_some();

        let message = if was_removed { "Removed" } else { "Updated" };
        let check_mark = console::style(console::Emoji("✔ ", "")).green();
        let env_fancy = env_name.fancy_display();

        // Output messages based on the type of changes
        if top_level_changes.is_empty() && !transitive_changes.is_empty() {
            eprintln!("{check_mark}Updated environment {env_fancy}.",);
        } else if top_level_changes.is_empty() && transitive_changes.is_empty() {
            eprintln!("{check_mark}Environment {env_fancy} was already up-to-date.");
        } else if top_level_changes.len() == 1 {
            let changes = console::style(top_level_changes[0].0.as_normalized()).green();
            let version_string = top_level_changes[0]
                .1
                .version_fancy_display()
                .map(|version| format!("={version}"))
                .unwrap_or_default();

            eprintln!(                "{check_mark}{message} package {changes}{version_string} in environment {env_fancy}."
            );
        } else if top_level_changes.len() > 1 {
            eprintln!(
                "{}{} packages in environment {}.",
                console::style(console::Emoji("✔ ", "")).green(),
                message,
                env_name.fancy_display()
            );
            for (package, install_change) in top_level_changes {
                let package_fancy = console::style(package.as_normalized()).green();
                let change_fancy = install_change
                    .version_fancy_display()
                    .map(|version| format!(" {version}"))
                    .unwrap_or_default();
                eprintln!("    - {package_fancy}{change_fancy}");
            }
        }
    }
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
    // If channel url contains channel config alias as a substring, don't use it as a URL
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

/// Figures out what the status is of the exposed binaries of the environment.
///
/// Returns a tuple of the exposed binaries to remove and the exposed binaries to add.
pub(crate) async fn get_expose_scripts_sync_status(
    bin_dir: &BinDir,
    env_dir: &EnvDir,
    mappings: &IndexSet<Mapping>,
) -> miette::Result<(Vec<GlobalExecutable>, IndexSet<ExposedName>)> {
    // Get all paths to the binaries from trampolines or scripts in the bin directory.
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

/// Check if all binaries were exposed, or if the user selected a subset of them.
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

    let auto_exposed =
        env_binaries_names_iter.all(|name| exposed_binaries_names.contains(&name.as_str()));

    auto_exposed
}

pub(crate) fn get_install_changes(
    install_transaction: Transaction<PrefixRecord, RepoDataRecord>,
) -> HashMap<PackageName, InstallChange> {
    install_transaction
        .operations
        .into_iter()
        .map(|transaction| match transaction {
            TransactionOperation::Install(package) => {
                let pkg_name = package.package_record.name;

                (
                    pkg_name,
                    InstallChange::Installed(package.package_record.version.version().clone()),
                )
            }
            TransactionOperation::Change { old, new } => {
                let old_pkg_version = old.repodata_record.package_record.version;
                let new_pkg_version = new.package_record.version;

                let pkg_name = new.package_record.name;

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

                (pkg_name, change)
            }
            TransactionOperation::Reinstall { old, new } => {
                let pkg_name = new.package_record.name;
                (
                    pkg_name,
                    InstallChange::Reinstalled(
                        old.repodata_record.package_record.version.version().clone(),
                        new.package_record.version.version().clone(),
                    ),
                )
            }
            TransactionOperation::Remove(package) => {
                let pkg_name = package.repodata_record.package_record.name;
                (pkg_name, InstallChange::Removed)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::global::trampoline::Configuration;

    use super::*;
    use rstest::rstest;
    use std::str::FromStr;
    use tempfile::tempdir;

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
        let dummy_conda_meta_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("global")
            .join("test_data")
            .join("conda-meta");
        // Find the package record
        let records = find_package_records(&dummy_conda_meta_path).await.unwrap();

        // Verify that the package record was found
        assert!(records
            .iter()
            .any(|rec| rec.repodata_record.package_record.name.as_normalized() == "python"));
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
            let expected = format!("{}.exe", exposed_name);
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
        let (to_remove, to_add) = get_expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
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
        let (to_remove, to_add) = get_expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
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
                let script_path = bin_dir.path().join(format!("{}.bat", script_name));
                let script = format!(
                    r#"
            @"{}" %*
            "#,
                    env_dir
                        .path()
                        .join("bin")
                        .join(format!("{}.exe", script_name))
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

        // Test to_remove and to_add to see if the legacy scripts are removed and trampolines are added
        let (to_remove, to_add) = get_expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
            .await
            .unwrap();
        assert!(to_remove.iter().all(|bin| !bin.is_trampoline()));
        assert_eq!(to_remove.len(), 2);
        assert_eq!(to_add.len(), 2);

        // Test to_remove when nothing should be exposed
        // it should remove all the legacy scripts and add nothing
        let (to_remove, to_add) =
            get_expose_scripts_sync_status(&bin_dir, &env_dir, &IndexSet::new())
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
        let (to_remove, to_add) = get_expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
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

        let (to_remove, to_add) = get_expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
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

        let manifest = Configuration::new(original_exe, bin_dir.path().join("bin"), None);
        let trampoline = Trampoline::new(
            ExposedName::from_str("test").unwrap(),
            bin_dir.path().to_path_buf(),
            manifest,
        );

        trampoline.save().await.unwrap();

        let (to_remove, to_add) = get_expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
            .await
            .unwrap();

        assert!(to_remove.is_empty());
        assert!(to_add.is_empty());

        // Test to_remove when nothing should be exposed
        let (mut to_remove, to_add) =
            get_expose_scripts_sync_status(&bin_dir, &env_dir, &IndexSet::new())
                .await
                .unwrap();
        assert_eq!(to_remove.len(), 1);

        assert_eq!(to_remove.pop().unwrap().exposed_name().to_string(), "test");
        assert!(to_add.is_empty());
    }
}
