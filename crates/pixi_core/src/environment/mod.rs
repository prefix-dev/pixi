pub(crate) mod conda_metadata;
mod conda_prefix;
pub mod list;
pub mod mount_sidecar;
pub use conda_prefix::{CondaPrefixUpdated, CondaPrefixUpdater, CondaPrefixUpdaterBuilder};
use dialoguer::theme::ColorfulTheme;
use futures::{FutureExt, StreamExt, TryStreamExt, stream};
use miette::{Context, IntoDiagnostic};
use pixi_consts::consts;
use pixi_git::credentials::store_credentials_from_url;
pub use pixi_install_pypi::{ContinuePyPIPrefixUpdate, on_python_interpreter_change};
use pixi_manifest::{FeaturesExt, HasWorkspaceManifest, PixiPlatform, PixiPlatformName};
use pixi_progress::await_in_progress;
use pixi_pypi_spec::PixiPypiSource;
pub use pixi_python_status::PythonStatus;
use pixi_spec::{GitSpec, PixiSpec};
use pixi_utils::EnvironmentFingerprint;
use pixi_utils::{prefix::Prefix, rlimit::try_increase_rlimit_to_sensible};
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_lock::{LockFile, LockedPackage};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    io::ErrorKind,
    path::{Path, PathBuf},
};
use xxhash_rust::xxh3::Xxh3;

use crate::workspace;
use crate::{
    Workspace,
    lock_file::{
        LockFileDerivedData, ReinstallPackages, UpdateLockFileOptions, UpdateMode,
        resolve_lock_platform_for,
    },
    workspace::{Environment, HasWorkspaceRef, grouped_environment::GroupedEnvironment},
};

/// Verify the location of the prefix folder is not changed so the applied
/// prefix path is still valid. Errors when there is a file system error or the
/// path does not align with the defined prefix. Returns false when the file is
/// not present.
pub async fn verify_prefix_location_unchanged(environment_dir: &Path) -> miette::Result<()> {
    let prefix_file = environment_dir
        .join(consts::CONDA_META_DIR)
        .join(consts::PREFIX_FILE_NAME);

    tracing::debug!(
        "verifying prefix location is unchanged, with prefix file: {}",
        prefix_file.display()
    );

    match fs_err::read_to_string(prefix_file.clone()) {
        // Not found is fine as it can be new or backwards compatible.
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        // Scream the error if we don't know it.
        Err(e) => {
            tracing::error!("failed to read prefix file: {}", prefix_file.display());
            Err(e).into_diagnostic()
        }
        // Check if the path in the file aligns with the current path.
        Ok(p) if prefix_file.starts_with(&p) => Ok(()),
        Ok(p) => {
            let path = Path::new(&p);
            prefix_location_changed(environment_dir, path.parent().unwrap_or(path)).await
        }
    }
}

/// Called when the prefix has moved to a new location.
///
/// Allows interactive users to delete the location and continue.
async fn prefix_location_changed(
    environment_dir: &Path,
    previous_dir: &Path,
) -> miette::Result<()> {
    let theme = ColorfulTheme {
        active_item_style: console::Style::new().for_stderr().magenta(),
        ..ColorfulTheme::default()
    };

    let user_value = dialoguer::Confirm::with_theme(&theme)
        .with_prompt(format!(
            "The environment directory seems have to moved! Environments are non-relocatable, moving them can cause issues.\n\n\t{} -> {}\n\nThis can be fixed by reinstall the environment from the lock file in the new location.\n\nDo you want to automatically recreate the environment?",
            previous_dir.display(),
            environment_dir.display()
        ))
        .report(false)
        .default(true)
        .interact_opt()
        .map_or(None, std::convert::identity);
    if user_value == Some(true) {
        await_in_progress("removing old environment", |_| {
            tokio::fs::remove_dir_all(environment_dir)
        })
        .await
        .into_diagnostic()
        .context("failed to remove old environment directory")?;
        Ok(())
    } else {
        Err(miette::diagnostic!(
            help = "Remove the environment directory, pixi will recreate it on the next run.",
            "The environment directory has moved from `{}` to `{}`. Environments are non-relocatable, moving them can cause issues.", previous_dir.display(), environment_dir.display()
        )
            .into())
    }
}

#[derive(Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct EnvironmentHash(String);

impl EnvironmentHash {
    /// Compute a hash that combines the project + locked-environment
    /// state.
    ///
    /// Used for **task** caching: a task's cached result is keyed on
    /// inputs the user can change without going through an install
    /// (manifest, lock file, env vars, activation scripts), so this
    /// flavour folds locked package URLs into the hash directly.
    ///
    /// The activation cache uses [`Self::for_activation`] instead --
    /// see the docs there for why URL-based hashing is too coarse for
    /// that use case.
    pub fn from_environment(
        run_environment: &workspace::Environment<'_>,
        input_environment_variables: &HashMap<String, Option<String>>,
        lock_file: &LockFile,
    ) -> Self {
        let mut hasher = Xxh3::new();
        Self::hash_common_inputs(&mut hasher, run_environment, input_environment_variables);

        // Hash the packages
        let mut urls = Vec::new();
        if let Some(env) = lock_file.environment(run_environment.name().as_str())
            && let Some(best) = run_environment.best_declared_platform()
            && let Some(lock_platform) = resolve_lock_platform_for(lock_file, best)
            && let Some(packages) = env.packages(lock_platform)
        {
            for package in packages {
                urls.push(package.location().to_string())
            }
        }
        urls.sort();
        urls.hash(&mut hasher);

        EnvironmentHash(format!("{:x}", hasher.finish()))
    }

    /// Compute the cache key for the activation env-var map.
    ///
    /// Activation results depend on:
    /// 1. Shell input env vars referenced by the activation scripts.
    /// 2. The project's activation scripts and activation env.
    /// 3. What is actually installed in the prefix.
    ///
    /// (3) is captured by `installed_fingerprint`, which is the
    /// per-record sha256 hash of every package in the prefix
    /// (binaries + built source-build artifacts) computed by
    /// [`pixi_utils::EnvironmentFingerprint`].
    ///
    /// We deliberately do **not** fold locked package URLs into this
    /// hash like [`Self::from_environment`] does: for source
    /// packages a URL is a stable path string that doesn't change
    /// when the source content (and therefore the built artifact)
    /// changes, so a URL-based key would falsely accept stale
    /// activation env vars after a source-rebuild. The fingerprint
    /// is the smallest authoritative summary of the prefix's
    /// content, so URLs add no signal beyond it.
    pub fn for_activation(
        run_environment: &workspace::Environment<'_>,
        input_environment_variables: &HashMap<String, Option<String>>,
        installed_fingerprint: &EnvironmentFingerprint,
    ) -> Self {
        let mut hasher = Xxh3::new();
        Self::hash_common_inputs(&mut hasher, run_environment, input_environment_variables);
        installed_fingerprint.as_str().hash(&mut hasher);
        EnvironmentHash(format!("{:x}", hasher.finish()))
    }

    /// Fold every input shared by both hash flavours into `hasher`:
    /// the shell input env vars (sorted by key for determinism),
    /// the activation scripts in declaration order, and the project
    /// activation env (sorted by key).
    fn hash_common_inputs(
        hasher: &mut Xxh3,
        run_environment: &workspace::Environment<'_>,
        input_environment_variables: &HashMap<String, Option<String>>,
    ) {
        let mut sorted_input_environment_variables: Vec<_> =
            input_environment_variables.iter().collect();
        sorted_input_environment_variables.sort_by_key(|(key, _)| *key);
        for (key, value) in sorted_input_environment_variables {
            key.hash(hasher);
            value.hash(hasher);
        }

        let activation_scripts =
            run_environment.activation_scripts(run_environment.best_declared_platform());
        for script in activation_scripts {
            script.hash(hasher);
        }

        let project_activation_env =
            run_environment.activation_env(run_environment.best_declared_platform());
        let mut env_vars: Vec<_> = project_activation_env.iter().collect();
        env_vars.sort_by_key(|(key, _)| *key);
        for (key, value) in env_vars {
            key.hash(hasher);
            value.hash(hasher);
        }
    }
}

impl Display for EnvironmentHash {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Cache key for the **quick-validate** fast path that decides whether an
/// installed prefix is still up-to-date.
///
/// The hash is stored in the prefix's `conda-meta/pixi` marker at install time.
/// On a later quick-validating command (`pixi run` / `shell` / `shell-hook`,
/// which pass [`UpdateMode::QuickValidate`] to [`LockFileDerivedData::prefix`])
/// the marker's hash is compared against a freshly computed one; a match
/// short-circuits the install entirely, so neither the packages nor the marker
/// itself are rewritten. `pixi install` always revalidates and never consults
/// this hash.
///
/// Because a match suppresses the marker rewrite, the hash must fold in every
/// input the marker records. Hence it covers not only the locked packages but
/// also the install platform's subdir and declared virtual packages (the
/// marker's `resolved_platform`): omit those and editing a virtual package such
/// as `__linux` leaves the recorded platform stale.
#[derive(Debug, Hash, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedEnvironmentHash(String);
impl LockedEnvironmentHash {
    pub(crate) fn from_environment(
        environment: rattler_lock::Environment,
        platform: Option<&PixiPlatform>,
    ) -> Self {
        let mut hasher = Xxh3::new();

        // Intentionally ignore `skipped` here: the quick-validate cache is only
        // used during runs, and should not vary based on transient install
        // filters.
        let lock_platform =
            platform.and_then(|p| resolve_lock_platform_for(environment.lock_file(), p));
        if let Some(packages) = lock_platform.and_then(|p| environment.packages(p)) {
            for package in packages {
                // Always has the url or path
                package.location().to_owned().to_string().hash(&mut hasher);

                match &package {
                    // A select set of fields are used to hash the package
                    LockedPackage::Conda(pack) => {
                        if let Some(record) = pack.record() {
                            if let Some(sha) = record.sha256 {
                                sha.hash(&mut hasher);
                            } else if let Some(md5) = record.md5 {
                                md5.hash(&mut hasher);
                            }
                        }
                    }
                    LockedPackage::Pypi(_) => {}
                }
            }
        }

        // Fold in the install platform recorded by the marker (see the type's
        // docs). Sort the virtual packages first so the hash is independent of
        // the order they appear in the manifest.
        if let Some(platform) = platform {
            platform.subdir().to_string().hash(&mut hasher);
            let mut virtual_packages: Vec<String> = platform
                .declared_virtual_packages()
                .iter()
                .map(ToString::to_string)
                .collect();
            virtual_packages.sort_unstable();
            for virtual_package in virtual_packages {
                virtual_package.hash(&mut hasher);
            }
        }

        LockedEnvironmentHash(format!("{:x}", hasher.finish()))
    }
}

impl LockedEnvironmentHash {
    /// Create an invalid hash for revalidation purposes
    pub fn invalid() -> Self {
        LockedEnvironmentHash("invalid-hash".to_string())
    }
}

/// The conda subdir plus the virtual packages that define a [`PixiPlatform`].
///
/// Stored instead of the platform's name so the full platform definition
/// survives: a synthesised rich-platform name can't be parsed back into its
/// virtual packages, and a bare subdir name drops them entirely.
#[derive(Serialize, Deserialize)]
pub struct PlatformData {
    /// The conda subdir this platform targets, e.g. `linux-64`.
    pub(crate) subdir: Platform,
    /// The virtual packages that define this platform.
    pub(crate) virtual_packages: Vec<GenericVirtualPackage>,
}

impl PlatformData {
    /// A platform definition from a subdir and the virtual packages that
    /// define it. Used by callers outside this module (e.g. `pixi global`)
    /// that compute the two fields themselves rather than from a
    /// [`PixiPlatform`].
    pub fn new(subdir: Platform, virtual_packages: Vec<GenericVirtualPackage>) -> Self {
        Self {
            subdir,
            virtual_packages,
        }
    }

    /// The conda subdir this platform targets, e.g. `linux-64`.
    pub fn subdir(&self) -> Platform {
        self.subdir
    }

    /// The virtual packages that define this platform.
    pub fn virtual_packages(&self) -> &[GenericVirtualPackage] {
        &self.virtual_packages
    }
}

impl From<&PixiPlatform> for PlatformData {
    fn from(platform: &PixiPlatform) -> Self {
        Self {
            subdir: platform.subdir(),
            virtual_packages: platform.declared_virtual_packages().to_vec(),
        }
    }
}

impl Display for PlatformData {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.subdir)?;
        if !self.virtual_packages.is_empty() {
            let packages = self
                .virtual_packages
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            write!(f, " [{packages}]")?;
        }
        Ok(())
    }
}

/// Information about the environment that was used to create the environment.
///
/// The install fingerprint that downstream caches key on lives in a
/// separate marker file written under the install lock by
/// `pixi_command_dispatcher::install_pixi_environment` and read
/// lock-free via [`pixi_utils::EnvironmentFingerprint::read`], so it
/// isn't part of this struct.
#[derive(Serialize, Deserialize)]
pub struct EnvironmentFile {
    /// The path to the manifest file that was used to create the environment.
    pub manifest_path: PathBuf,
    /// The name of the environment.
    pub environment_name: String,
    /// The version of the pixi that was used to create the environment.
    pub pixi_version: String,
    /// The hash of the lock file that was used to create the environment.
    /// `pixi global` environments aren't validated against a workspace lock
    /// file, so they record [`LockedEnvironmentHash::invalid`] here.
    pub environment_lock_file_hash: LockedEnvironmentHash,
    /// The platform the environment was resolved with (subdir + the virtual
    /// packages the workspace declared for it). `None` on environments written
    /// by an older pixi, or when no declared platform runs on this machine.
    #[serde(default)]
    pub resolved_platform: Option<PlatformData>,
    /// The minimum platform the installed packages actually require (the subdir
    /// plus only the virtual packages some resolved dependency depends on). Can
    /// be weaker than [`Self::resolved_platform`]. `None` as above.
    #[serde(default)]
    pub minimum_supported_platform: Option<PlatformData>,
}

/// The path to the environment file in the `conda-meta` directory of the
/// environment.
fn environment_file_path(environment_dir: &Path) -> PathBuf {
    environment_dir
        .join(consts::CONDA_META_DIR)
        .join(consts::ENVIRONMENT_FILE_NAME)
}

/// Write information about the environment to a file in the environment
/// directory. Used by the prefix updating to validate if it needs to be
/// updated.
pub fn write_environment_file(
    environment_dir: &Path,
    env_file: EnvironmentFile,
) -> miette::Result<PathBuf> {
    let path = environment_file_path(environment_dir);

    let parent = path
        .parent()
        .expect("There should already be a conda-meta folder");

    match fs_err::create_dir_all(parent).into_diagnostic() {
        Ok(_) => {
            // Using json as it's easier to machine read it.
            let contents = serde_json::to_string_pretty(&env_file).into_diagnostic()?;
            match fs_err::write(&path, contents).into_diagnostic() {
                Ok(_) => {
                    tracing::debug!("Wrote environment file to: {:?}", path);
                }
                Err(e) => tracing::debug!(
                    "Unable to write environment file to: {:?} => {:?}",
                    path,
                    e.root_cause().to_string()
                ),
            };
            Ok(path)
        }
        Err(e) => {
            tracing::debug!("Unable to create conda-meta folder to: {:?}", path);
            Err(e)
        }
    }
}

/// Reading the environment file of the environment.
/// Removing it if it's not valid.
pub(crate) fn read_environment_file(
    environment_dir: &Path,
) -> miette::Result<Option<EnvironmentFile>> {
    let path = environment_file_path(environment_dir);

    let contents = match fs_err::read_to_string(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tracing::debug!("Environment file not yet found at: {:?}", path);
            return Ok(None);
        }
        Err(e) => {
            tracing::debug!(
                "Failed to read environment file at: {:?}, error: {}, will try to remove it.",
                path,
                e
            );
            let _ = fs_err::remove_file(&path);
            return Err(e).into_diagnostic();
        }
    };
    let env_file: EnvironmentFile = match serde_json::from_str(&contents) {
        Ok(env_file) => env_file,
        Err(e) => {
            tracing::debug!(
                "Failed to read environment file at: {:?}, error: {}, will try to remove it.",
                path,
                e
            );
            let _ = fs_err::remove_file(&path);
            return Ok(None);
        }
    };

    Ok(Some(env_file))
}

/// Runs the following checks to make sure the project is in a sane state:
///     1. It verifies that the prefix location is unchanged.
///     2. It verifies that the system requirements are met.
///     3. It verifies the absence of the `env` folder.
///     4. It verifies that the prefix contains a `.gitignore` file.
pub async fn sanity_check_workspace(project: &Workspace) -> miette::Result<()> {
    // Sanity check of prefix location
    verify_prefix_location_unchanged(project.environments_dir().as_path()).await?;

    // TODO: remove on a 1.0 release
    // Check for old `env` folder as we moved to `envs` in 0.13.0
    let old_pixi_env_dir = project.pixi_dir().join("env");
    if old_pixi_env_dir.exists() {
        tracing::warn!(
            "The `{}` folder is deprecated, please remove it as we now use the `{}` folder",
            old_pixi_env_dir.display(),
            consts::ENVIRONMENTS_DIR
        );
    }

    ensure_pixi_directory_and_gitignore(project.pixi_dir().as_path()).await?;

    Ok(())
}

/// Extract [`GitSpec`] requirements from the project dependencies.
pub fn extract_git_requirements_from_workspace(project: &Workspace) -> Vec<GitSpec> {
    let mut requirements = Vec::new();

    for env in project.environments() {
        let env_platforms = env.platforms();
        for platform in env_platforms {
            let platform = project
                .workspace
                .value
                .workspace
                .platform_by_name(&platform);
            let dependencies = env.combined_dependencies(platform);
            let pypi_dependencies = env.pypi_dependencies(platform);
            for (_, dep_spec) in dependencies {
                for spec in dep_spec {
                    if let PixiSpec::Git(spec) = spec {
                        requirements.push(*spec);
                    }
                }
            }

            for (_, pypi_spec) in pypi_dependencies {
                for spec in pypi_spec {
                    if let PixiPypiSource::Git { git, .. } = &spec.source {
                        requirements.push(git.clone());
                    }
                }
            }
        }
    }

    requirements
}

/// Store credentials from [`GitSpec`] requirements.
pub fn store_credentials_from_requirements(git_requirements: Vec<GitSpec>) {
    for spec in git_requirements {
        store_credentials_from_url(&spec.git);
    }
}

/// Extract any credentials that are defined on the project dependencies
/// themselves. While we don't store plaintext credentials in the `pixi.lock`,
/// we do respect credentials that are defined in the `pixi.toml` or
/// `pyproject.toml`.
pub async fn store_credentials_from_project(project: &Workspace) -> miette::Result<()> {
    for env in project.environments() {
        let env_platforms = env.platforms();
        for platform in env_platforms {
            // A feature may name a platform absent from `workspace.platforms`;
            // pass the `Option` through (a miss means no platform overrides).
            let platform = project
                .workspace
                .value
                .workspace
                .platform_by_name(&platform);
            let dependencies = env.combined_dependencies(platform);
            for (_, dep_spec) in dependencies {
                for spec in dep_spec {
                    if let PixiSpec::Git(spec) = spec {
                        store_credentials_from_url(&spec.git);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Create a file at the given path with the given contents if it does not exist.
/// If the file already exists, it does nothing.
/// If the file cannot be created due to a read-only filesystem,
/// we'll ignore the error as it's not that important to the function of pixi.
async fn best_effort_write_file_if_missing(
    path: &Path,
    contents: &str,
    error_message: &str,
) -> miette::Result<()> {
    if !path.exists() {
        match tokio::fs::write(path, contents).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == ErrorKind::ReadOnlyFilesystem => {
                tracing::debug!("Failed to create file at: {}, error: {}", path.display(), e);
                Ok(())
            }
            Err(e) => Err(e)
                .into_diagnostic()
                .wrap_err(format!("{error_message} {}", path.display())),
        }?;
    }
    Ok(())
}

/// Ensure that the `.pixi/` directory exists and contains a `.gitignore` file.
/// If the directory doesn't exist, create it.
/// If the `.gitignore` file doesn't exist, create it with a '*' pattern.
/// Also creates a `.condapackageignore` file to exclude the `.pixi` directory
/// from builds.
async fn ensure_pixi_directory_and_gitignore(pixi_dir: &Path) -> miette::Result<()> {
    let gitignore_path = pixi_dir.join(".gitignore");
    let condapackageignore_path = pixi_dir.join(".condapackageignore");

    // Create the `.pixi/` directory if it doesn't exist
    if !pixi_dir.exists() {
        tokio::fs::create_dir_all(&pixi_dir)
            .await
            .into_diagnostic()
            .wrap_err(format!(
                "Failed to create .pixi/ directory at {}",
                pixi_dir.display()
            ))?;
    }

    best_effort_write_file_if_missing(
        &gitignore_path,
        "*\n!config.toml\n",
        "Failed to create .gitignore file at",
    )
    .await?;

    best_effort_write_file_if_missing(
        &condapackageignore_path,
        ".pixi\n!.pixi/config.toml\n",
        "Failed to create .condapackageignore file at",
    )
    .await?;

    Ok(())
}

/// Specifies how the lock file should be updated.
#[derive(Debug, Default, PartialEq, Eq, Copy, Clone, Deserialize, Serialize)]
pub enum LockFileUsage {
    /// Update the lock file if it is out of date.
    #[default]
    Update,
    /// Don't update the lock file, but do check if it is out of date
    Locked,
    /// Don't update the lock file and don't check if it is out of date
    Frozen,
    /// Don't update the lock file, but don't check if it is out of date
    DryRun,
}

impl LockFileUsage {
    /// Returns true if the process should error when the lock file
    pub(crate) fn allow_updates(self) -> bool {
        !matches!(self, LockFileUsage::Locked)
    }

    /// Returns true if the lock file should be checked if it is out of date.
    pub(crate) fn should_check_if_out_of_date(self) -> bool {
        match self {
            LockFileUsage::Update | LockFileUsage::Locked | LockFileUsage::DryRun => true,
            LockFileUsage::Frozen => false,
        }
    }
}

/// Options to select a subset of packages to install or skip.
#[derive(Debug, Default, Clone)]
pub struct InstallFilter {
    /// Packages to skip directly but still traverse through their dependencies
    pub skip_direct: Vec<String>,
    /// Packages to skip together with their dependencies (hard stop)
    pub skip_with_deps: Vec<String>,
    /// Target one or more packages (and their deps) to install; empty means no targeting
    pub target_packages: Vec<String>,
}

impl InstallFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn skip_direct(mut self, packages: impl Into<Vec<String>>) -> Self {
        self.skip_direct = packages.into();
        self
    }

    pub fn skip_with_deps(mut self, packages: impl Into<Vec<String>>) -> Self {
        self.skip_with_deps = packages.into();
        self
    }

    pub fn target_packages(mut self, packages: impl Into<Vec<String>>) -> Self {
        self.target_packages = packages.into();
        self
    }

    /// Is the filter currently active
    pub fn filter_active(&self) -> bool {
        !self.skip_direct.is_empty()
            || !self.skip_with_deps.is_empty()
            || !self.target_packages.is_empty()
    }
}

/// Update the prefix if it doesn't exist or if it is not up-to-date.
///
/// To updated multiple prefixes at once, use [`get_update_lock_file_and_prefixes`].
pub async fn get_update_lock_file_and_prefix<'env>(
    environment: &Environment<'env>,
    progress: Option<std::sync::Arc<pixi_reporters::TopLevelProgress>>,
    update_mode: UpdateMode,
    update_lock_file_options: UpdateLockFileOptions,
    reinstall_packages: ReinstallPackages,
    filter: &InstallFilter,
) -> miette::Result<(LockFileDerivedData<'env>, Prefix)> {
    let (lock_file, prefixes) = get_update_lock_file_and_prefixes(
        std::slice::from_ref(environment),
        None,
        progress.clone(),
        update_mode,
        update_lock_file_options,
        reinstall_packages,
        filter,
    )
    .await?;
    Ok((
        lock_file,
        prefixes
            .into_iter()
            .next()
            .expect("must be at least one prefix"),
    ))
}

/// Update all the specified prefixes if it doesn't exist or if it is not
/// up-to-date.
///
/// When `target_platform` is `Some`, every environment must list that
/// platform; the install path then targets it directly without running
/// the host-virtual-package satisfaction check. That's how
/// `pixi install --platform <name>` materialises an environment for a
/// subdir the local machine can't actually run.
pub async fn get_update_lock_file_and_prefixes<'env>(
    environments: &[Environment<'env>],
    target_platform: Option<&PixiPlatformName>,
    progress: Option<std::sync::Arc<pixi_reporters::TopLevelProgress>>,
    update_mode: UpdateMode,
    update_lock_file_options: UpdateLockFileOptions,
    reinstall_packages: ReinstallPackages,
    filter: &InstallFilter,
) -> miette::Result<(LockFileDerivedData<'env>, Vec<Prefix>)> {
    if environments.is_empty() {
        return Err(miette::miette!("No environments provided to install."));
    }

    let workspace = environments[0].workspace();

    let no_install = update_lock_file_options.no_install;
    for env in environments {
        // A `--platform` the environment doesn't list is a membership error.
        // With no platform requested, defer to the install path's minimum fallback.
        if !no_install
            && env
                .named_or_best_declared_platform(target_platform)
                .is_none()
            && let Some(name) = target_platform
        {
            return Err(miette::miette!(
                "platform '{}' is not part of environment '{}'",
                name,
                env.name(),
            ));
        }
        if !no_install {
            env.emit_emulation_warning();
        }
    }

    // Every environment lists the pinned platform (checked above), so a
    // cross-target `--platform` (a subdir this host can't run) only warns
    // now -- after the clear membership error, never before it.
    if !no_install
        && target_platform.is_some()
        && let Some(platform) = environments[0].named_or_best_declared_platform(target_platform)
    {
        let current = Platform::current();
        let subdir = platform.subdir();
        if !workspace
            .workspace_manifest()
            .workspace
            .candidate_subdirs(current)
            .contains(&subdir)
        {
            tracing::warn!(
                "installing for platform '{}' (subdir '{subdir}'), which this \
                 machine ('{current}') can not run -- packages will be downloaded \
                 and extracted but won't be executable here",
                platform.name(),
            );
        }
    }

    // Make sure the project is in a sane state
    sanity_check_workspace(workspace).await?;

    // Store the git credentials from the git requirements
    let requirements = extract_git_requirements_from_workspace(workspace);
    store_credentials_from_requirements(requirements);

    // Ensure that the lock file is up-to-date
    let mut lock_file = workspace
        .update_lock_file(
            progress.clone(),
            UpdateLockFileOptions {
                lock_file_usage: update_lock_file_options.lock_file_usage,
                no_install,
                max_concurrent_solves: update_lock_file_options.max_concurrent_solves,
                ..Default::default()
            },
        )
        .await?
        .0;
    // Pin the override so the downstream prefix helpers see it without a
    // fresh parameter on every call.
    lock_file.target_platform = target_platform.cloned();

    // Get the prefix from the lock file.
    let lock_file_ref = &lock_file;
    let reinstall_packages = &reinstall_packages;
    let prefixes = stream::iter(environments.iter())
        .map(move |env| {
            if no_install {
                std::future::ready(Ok(Prefix::new(env.dir()))).left_future()
            } else {
                lock_file_ref
                    .prefix(env, update_mode, reinstall_packages, filter)
                    .right_future()
            }
        })
        .buffer_unordered(environments.len())
        .try_collect()
        .await?;

    Ok((lock_file, prefixes))
}

pub type PerEnvironment<'p, T> = HashMap<Environment<'p>, T>;
pub type PerGroup<'p, T> = HashMap<GroupedEnvironment<'p>, T>;
pub type PerEnvironmentAndPlatform<'p, T> = PerEnvironment<'p, HashMap<PixiPlatformName, T>>;
pub type PerGroupAndPlatform<'p, T> = PerGroup<'p, HashMap<PixiPlatformName, T>>;

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_conda_types::{PackageName, Version};

    use super::*;

    /// A marker file written by an older pixi has no platform fields. It must
    /// still deserialize (with both fields `None`) so a pixi upgrade doesn't
    /// invalidate every installed prefix.
    #[test]
    fn environment_file_without_platforms_deserializes() {
        let json = r#"{
            "manifest_path": "/ws/pixi.toml",
            "environment_name": "default",
            "pixi_version": "0.1.0",
            "environment_lock_file_hash": "deadbeef"
        }"#;
        let parsed: EnvironmentFile = serde_json::from_str(json).expect("legacy file parses");
        assert!(parsed.resolved_platform.is_none());
        assert!(parsed.minimum_supported_platform.is_none());
    }

    /// A linux-64 lock environment with no packages, so the quick-validate
    /// hash varies only with the platform passed to `from_environment`.
    fn empty_linux_lock() -> rattler_lock::LockFile {
        let mut builder = rattler_lock::LockFile::builder()
            .with_platforms(vec![rattler_lock::PlatformData {
                name: rattler_lock::PlatformName::try_from("linux-64").unwrap(),
                subdir: Platform::Linux64,
                virtual_packages: vec![],
            }])
            .unwrap();
        builder.set_channels("default", Vec::<rattler_lock::Channel>::new());
        builder.finish()
    }

    fn linux_platform_with_kernel(version: &str) -> PixiPlatform {
        let linux = GenericVirtualPackage {
            name: PackageName::from_str("__linux").unwrap(),
            version: Version::from_str(version).unwrap(),
            build_string: String::new(),
        };
        PixiPlatform::new(
            PixiPlatformName::try_from("linux-box").unwrap(),
            Platform::Linux64,
            vec![linux],
        )
        .unwrap()
    }

    /// The quick-validate hash must change when the install platform's declared
    /// virtual packages change, even though the locked package set is identical.
    /// Otherwise editing `__linux` in the manifest leaves the `conda-meta/pixi`
    /// marker's resolved platform stale (the bug this test guards).
    #[test]
    fn hash_changes_when_platform_virtual_packages_change() {
        let lock_file = empty_linux_lock();
        let environment = lock_file.environment("default").unwrap();

        let old = linux_platform_with_kernel("5.9");
        let new = linux_platform_with_kernel("7.0");

        let hash_old = LockedEnvironmentHash::from_environment(environment, Some(&old));
        let hash_new = LockedEnvironmentHash::from_environment(environment, Some(&new));
        assert_ne!(hash_old, hash_new);

        // The same platform hashes identically, so the marker is rewritten only
        // once after a change rather than on every subsequent run.
        let hash_old_again = LockedEnvironmentHash::from_environment(environment, Some(&old));
        assert_eq!(hash_old, hash_old_again);
    }

    /// The hash is independent of the order virtual packages are declared in, so
    /// reordering them in the manifest doesn't trigger a needless reinstall.
    #[test]
    fn hash_independent_of_virtual_package_order() {
        let lock_file = empty_linux_lock();
        let environment = lock_file.environment("default").unwrap();

        let gvp = |name: &str, version: &str| GenericVirtualPackage {
            name: PackageName::from_str(name).unwrap(),
            version: Version::from_str(version).unwrap(),
            build_string: String::new(),
        };
        let make = |packages: Vec<GenericVirtualPackage>| {
            PixiPlatform::new(
                PixiPlatformName::try_from("linux-box").unwrap(),
                Platform::Linux64,
                packages,
            )
            .unwrap()
        };
        let one = make(vec![gvp("__linux", "7.0"), gvp("__cuda", "12")]);
        let other = make(vec![gvp("__cuda", "12"), gvp("__linux", "7.0")]);

        assert_eq!(
            LockedEnvironmentHash::from_environment(environment, Some(&one)),
            LockedEnvironmentHash::from_environment(environment, Some(&other)),
        );
    }

    /// `PlatformData` stores the platform's composition (subdir + declared
    /// virtual packages), not its name -- a custom rich-platform name carries
    /// none of the virtual packages, so the name alone would be lossy.
    #[test]
    fn platform_data_captures_composition_not_name() {
        let cuda = GenericVirtualPackage {
            name: PackageName::from_str("__cuda").unwrap(),
            version: Version::from_str("12").unwrap(),
            build_string: String::new(),
        };
        let platform = PixiPlatform::new(
            PixiPlatformName::try_from("gpu-box").unwrap(),
            Platform::Linux64,
            vec![cuda.clone()],
        )
        .unwrap();

        let data = PlatformData::from(&platform);
        assert_eq!(data.subdir, Platform::Linux64);
        assert!(data.virtual_packages.contains(&cuda));

        // The composition survives a JSON round-trip; the custom name is not
        // part of the stored data.
        let json = serde_json::to_string(&data).unwrap();
        assert!(!json.contains("gpu-box"));
        let restored: PlatformData = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.subdir, Platform::Linux64);
        assert!(restored.virtual_packages.contains(&cuda));
    }
}
