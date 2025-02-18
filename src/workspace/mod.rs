mod discovery;
mod environment;
pub mod errors;
pub mod grouped_environment;
mod has_project_ref;
mod repodata;
mod solve_group;
pub mod virtual_packages;
mod workspace_mut;

#[cfg(not(windows))]
use std::os::unix::fs::symlink;
use std::{
    collections::{HashMap, HashSet},
    fmt::{Debug, Formatter},
    hash::Hash,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_once_cell::OnceCell as AsyncCell;
pub use discovery::{DiscoveryStart, WorkspaceLocator, WorkspaceLocatorError};
pub use environment::Environment;
pub use has_project_ref::HasWorkspaceRef;
use indexmap::Equivalent;
use itertools::Itertools;
use miette::IntoDiagnostic;
use once_cell::sync::OnceCell;
use pep508_rs::Requirement;
use pixi_config::Config;
use pixi_consts::consts;
use pixi_manifest::{
    pypi::PyPiPackageName, AssociateProvenance, EnvironmentName, Environments,
    HasWorkspaceManifest, LoadManifestsError, ManifestProvenance, Manifests, PackageManifest,
    SpecType, WithProvenance, WithWarnings, WorkspaceManifest,
};
use pixi_spec::SourceSpec;
use pixi_utils::reqwest::build_reqwest_clients;
use pypi_mapping::{ChannelName, CustomMapping, MappingLocation, MappingSource};
use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, PackageName, Platform};
use rattler_lock::{LockFile, LockedPackageRef};
use rattler_networking::s3_middleware;
use rattler_repodata_gateway::Gateway;
use reqwest_middleware::ClientWithMiddleware;
pub use solve_group::SolveGroup;
use url::{ParseError, Url};
pub use workspace_mut::WorkspaceMut;
use xxhash_rust::xxh3::xxh3_64;

use crate::{
    activation::{initialize_env_variables, CurrentEnvVarBehavior},
    diff::LockFileDiff,
    lock_file::filter_lock_file,
};

static CUSTOM_TARGET_DIR_WARN: OnceCell<()> = OnceCell::new();

/// The dependency types we support
#[derive(Debug, Copy, Clone)]
pub enum DependencyType {
    CondaDependency(SpecType),
    PypiDependency,
}

impl DependencyType {
    /// Convert to a name used in the manifest
    pub(crate) fn name(&self) -> &'static str {
        match self {
            DependencyType::CondaDependency(dep) => dep.name(),
            DependencyType::PypiDependency => consts::PYPI_DEPENDENCIES,
        }
    }
}

/// Environment variable cache for different activations
#[derive(Debug, Clone)]
pub struct EnvironmentVars {
    clean: Arc<AsyncCell<HashMap<String, String>>>,
    pixi_only: Arc<AsyncCell<HashMap<String, String>>>,
    full: Arc<AsyncCell<HashMap<String, String>>>,
}

impl EnvironmentVars {
    /// Create a new instance with empty AsyncCells
    pub(crate) fn new() -> Self {
        Self {
            clean: Arc::new(AsyncCell::new()),
            pixi_only: Arc::new(AsyncCell::new()),
            full: Arc::new(AsyncCell::new()),
        }
    }

    /// Get the clean environment variables
    pub(crate) fn clean(&self) -> &Arc<AsyncCell<HashMap<String, String>>> {
        &self.clean
    }

    /// Get the pixi_only environment variables
    pub(crate) fn pixi_only(&self) -> &Arc<AsyncCell<HashMap<String, String>>> {
        &self.pixi_only
    }

    /// Get the full environment variables
    pub(crate) fn full(&self) -> &Arc<AsyncCell<HashMap<String, String>>> {
        &self.full
    }
}

/// List of packages that are not following the semver versioning scheme
/// but will use the minor version by default when adding a dependency.
// Don't forget to add to the docstring if you add a package here!
const NON_SEMVER_PACKAGES: [&str; 11] = [
    "python", "rust", "julia", "gcc", "gxx", "gfortran", "nodejs", "deno", "r", "r-base", "perl",
];

/// The pixi workspace, this main struct to interact with a workspace.
///
/// This structs holds manifests of the workspace and optionally the current
/// package. The current package is considered the package the user is
/// interacting with.
///
/// The struct also holds several cached values that can be used throughout the
/// program like an HTTP request client and configuration.
#[derive(Clone)]
pub struct Workspace {
    /// Root folder of the workspace
    root: PathBuf,

    /// Reqwest client shared for this workspace.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    // TODO: once https://github.com/rust-lang/rust/issues/109737 is stabilized, switch to OnceLock
    client: OnceCell<(reqwest::Client, ClientWithMiddleware)>,

    /// The repodata gateway to use for answering queries about repodata.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    // TODO: once https://github.com/rust-lang/rust/issues/109737 is stabilized, switch to OnceLock
    repodata_gateway: OnceCell<Gateway>,

    /// The manifest for the workspace
    pub workspace: WithProvenance<WorkspaceManifest>,

    /// The manifest of the "current" package. This is the package from which
    /// the workspace was discovered. This might be `None` if no package was
    /// discovered on the current path.
    pub package: Option<WithProvenance<PackageManifest>>,

    /// The environment variables that are activated when the environment is
    /// activated. Cached per environment, for both clean and normal
    env_vars: HashMap<EnvironmentName, EnvironmentVars>,

    /// The cache that contains mapping
    mapping_source: OnceCell<MappingSource>,

    /// The global configuration as loaded from the config file(s)
    config: Config,
    /// The S3 configuration
    s3_config: HashMap<String, s3_middleware::S3Config>,
}

impl Debug for Workspace {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project")
            .field("root", &self.root)
            .field("workspace", &self.workspace)
            .field("package", &self.package)
            .finish()
    }
}

pub type PypiDeps = indexmap::IndexMap<
    PyPiPackageName,
    (Requirement, Option<pixi_manifest::PypiDependencyLocation>),
>;

pub type MatchSpecs = indexmap::IndexMap<PackageName, (MatchSpec, SpecType)>;
pub type SourceSpecs = indexmap::IndexMap<PackageName, (SourceSpec, SpecType)>;

impl Workspace {
    /// Constructs a new instance from an internal manifest representation
    pub(crate) fn from_manifests(manifest: Manifests) -> Self {
        let env_vars = Workspace::init_env_vars(&manifest.workspace.value.environments);
        // Canonicalize the root path
        let root = &manifest.workspace.provenance.path;
        let root = dunce::canonicalize(root).unwrap_or(root.to_path_buf());
        // Take the parent after canonicalizing to ensure this works even when the manifest
        let root = root
            .parent()
            .expect("manifest path should always have a parent")
            .to_owned();

        let s3_options = manifest.workspace.value.workspace.s3_options.clone();
        let s3_config = s3_options
            .unwrap_or_default()
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    s3_middleware::S3Config::Custom {
                        endpoint_url: value.endpoint_url.clone(),
                        region: value.region.clone(),
                        force_path_style: value.force_path_style,
                    },
                )
            })
            .collect::<HashMap<String, s3_middleware::S3Config>>();

        let config = Config::load(&root);
        Self {
            root,
            client: Default::default(),
            workspace: manifest.workspace,
            package: manifest.package,
            env_vars,
            mapping_source: Default::default(),
            config,
            s3_config,
            repodata_gateway: Default::default(),
        }
    }

    /// Loads a project from manifest file. The `manifest_path` is expected to
    /// be a workspace manifest.
    pub fn from_path(manifest_path: &Path) -> Result<Self, LoadManifestsError> {
        let WithWarnings {
            value: manifests, ..
        } = Manifests::from_workspace_manifest_path(manifest_path.to_path_buf())?;
        Ok(Self::from_manifests(manifests))
    }

    /// Constructs a workspace from source loaded from a specific location.
    pub fn from_str(manifest_path: &Path, content: &str) -> Result<Self, LoadManifestsError> {
        let WithWarnings {
            value: manifests, ..
        } = Manifests::from_workspace_source(
            content.with_provenance(ManifestProvenance::from_path(manifest_path.to_path_buf())?),
        )?;
        Ok(Self::from_manifests(manifests))
    }

    /// Initialize empty map of environments variables
    fn init_env_vars(environments: &Environments) -> HashMap<EnvironmentName, EnvironmentVars> {
        environments
            .iter()
            .map(|environment| (environment.name.clone(), EnvironmentVars::new()))
            .collect()
    }

    pub fn env_vars(&self) -> &HashMap<EnvironmentName, EnvironmentVars> {
        &self.env_vars
    }

    pub(crate) fn with_cli_config<C>(mut self, config: C) -> Self
    where
        C: Into<Config>,
    {
        self.config = self.config.merge_config(config.into());
        self
    }

    pub fn modify(self) -> Result<WorkspaceMut, LoadManifestsError> {
        WorkspaceMut::new(self)
    }

    /// Returns the name of the workspace
    pub fn name(&self) -> &str {
        &self.workspace.value.workspace.name
    }

    /// Returns the root directory of the workspace
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the pixi directory of the workspace [consts::PIXI_DIR]
    pub fn pixi_dir(&self) -> PathBuf {
        self.root.join(consts::PIXI_DIR)
    }

    /// Create the detached-environments path for this project if it is set in
    /// the config
    fn detached_environments_path(&self) -> Option<PathBuf> {
        if let Ok(Some(detached_environments_path)) = self.config().detached_environments().path() {
            Some(detached_environments_path.join(format!(
                "{}-{}",
                self.name(),
                xxh3_64(self.root.to_string_lossy().as_bytes())
            )))
        } else {
            None
        }
    }

    /// Returns the default environment directory without interacting with
    /// config.
    pub(crate) fn default_environments_dir(&self) -> PathBuf {
        self.pixi_dir().join(consts::ENVIRONMENTS_DIR)
    }

    /// Returns the environment directory
    pub(crate) fn environments_dir(&self) -> PathBuf {
        let default_envs_dir = self.default_environments_dir();

        // Early out if detached-environments is not set
        if self.config().detached_environments().is_false() {
            return default_envs_dir;
        }

        // If the detached-environments path is set, use it instead of the default
        // directory.
        if let Some(detached_environments_path) = self.detached_environments_path() {
            let detached_environments_path =
                detached_environments_path.join(consts::ENVIRONMENTS_DIR);
            let _ = CUSTOM_TARGET_DIR_WARN.get_or_init(|| {
                if default_envs_dir.exists() && !default_envs_dir.is_symlink() {
                    tracing::warn!(
                        "Environments found in '{}', this will be ignored and the environment will be installed in the 'detached-environments' directory: '{}'. It's advised to remove the {} folder from the default directory to avoid confusion{}.",
                        default_envs_dir.display(),
                        detached_environments_path.parent().expect("path should have parent").display(),
                        format!("{}/{}", consts::PIXI_DIR, consts::ENVIRONMENTS_DIR),
                        if cfg!(windows) { "" } else { " as a symlink can be made, please re-install after removal." }
                    );
                } else {
                    #[cfg(not(windows))]
                    create_symlink(&detached_environments_path, &default_envs_dir);
                }

                #[cfg(windows)]
                write_warning_file(&default_envs_dir, &detached_environments_path);
            });

            return detached_environments_path;
        }

        tracing::debug!(
            "Using default root directory: `{}` as environments directory.",
            default_envs_dir.display()
        );

        default_envs_dir
    }

    /// Returns the default solve group environments directory, without
    /// interacting with config
    pub(crate) fn default_solve_group_environments_dir(&self) -> PathBuf {
        self.pixi_dir().join(consts::SOLVE_GROUP_ENVIRONMENTS_DIR)
    }

    /// Returns the solve group environments directory
    pub(crate) fn solve_group_environments_dir(&self) -> PathBuf {
        // If the detached-environments path is set, use it instead of the default
        // directory.
        if let Some(detached_environments_path) = self.detached_environments_path() {
            return detached_environments_path.join(consts::SOLVE_GROUP_ENVIRONMENTS_DIR);
        }
        self.default_solve_group_environments_dir()
    }

    /// Returns the path to the lock file of the project
    /// [consts::PROJECT_LOCK_FILE]
    pub(crate) fn lock_file_path(&self) -> PathBuf {
        self.root.join(consts::PROJECT_LOCK_FILE)
    }

    /// Returns the default environment of the project.
    pub fn default_environment(&self) -> Environment<'_> {
        Environment::new(self, self.workspace.value.default_environment())
    }

    /// Returns the environment with the given name or `None` if no such
    /// environment exists.
    pub fn environment<Q>(&self, name: &Q) -> Option<Environment<'_>>
    where
        Q: ?Sized + Hash + Equivalent<EnvironmentName>,
    {
        Some(Environment::new(
            self,
            self.workspace.value.environment(name)?,
        ))
    }

    /// Returns the environments in this project.
    pub(crate) fn environments(&self) -> Vec<Environment> {
        self.workspace
            .value
            .environments
            .iter()
            .map(|env| Environment::new(self, env))
            .collect()
    }

    /// Returns an environment in this project based on a name or an environment
    /// variable.
    pub fn environment_from_name_or_env_var(
        &self,
        name: Option<String>,
    ) -> miette::Result<Environment> {
        let environment_name = EnvironmentName::from_arg_or_env_var(name).into_diagnostic()?;
        self.environment(&environment_name)
            .ok_or_else(|| miette::miette!("unknown environment '{environment_name}'"))
    }

    /// Returns all the solve groups in the project.
    pub(crate) fn solve_groups(&self) -> Vec<SolveGroup> {
        self.workspace
            .value
            .solve_groups
            .iter()
            .map(|group| SolveGroup {
                workspace: self,
                solve_group: group,
            })
            .collect()
    }

    /// Returns the solve group with the given name or `None` if no such group
    /// exists.
    pub(crate) fn solve_group(&self, name: &str) -> Option<SolveGroup> {
        self.workspace
            .value
            .solve_groups
            .find(name)
            .map(|group| SolveGroup {
                workspace: self,
                solve_group: group,
            })
    }

    /// Returns the reqwest client used for http networking
    pub(crate) fn client(&self) -> miette::Result<&reqwest::Client> {
        Ok(&self.client_and_authenticated_client()?.0)
    }

    /// Create an authenticated reqwest client for this project
    /// use authentication from `rattler_networking`
    pub fn authenticated_client(&self) -> miette::Result<&ClientWithMiddleware> {
        Ok(&self.client_and_authenticated_client()?.1)
    }

    fn client_and_authenticated_client(
        &self,
    ) -> miette::Result<&(reqwest::Client, ClientWithMiddleware)> {
        self.client.get_or_try_init(|| {
            build_reqwest_clients(Some(&self.config), Some(self.s3_config.clone()))
        })
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    /// Construct a [`ChannelConfig`] that is specific to this project. This
    /// ensures that the root directory is set correctly.
    pub fn channel_config(&self) -> ChannelConfig {
        ChannelConfig {
            root_dir: self.root.clone(),
            ..self.config.global_channel_config().clone()
        }
    }

    pub(crate) fn task_cache_folder(&self) -> PathBuf {
        self.pixi_dir().join(consts::TASK_CACHE_DIR)
    }

    pub(crate) fn activation_env_cache_folder(&self) -> PathBuf {
        self.pixi_dir().join(consts::ACTIVATION_ENV_CACHE_DIR)
    }

    /// Returns what pypi mapping configuration we should use.
    /// It can be a custom one  in following format : conda_name: pypi_name
    /// Or we can use our self-hosted
    pub fn pypi_name_mapping_source(&self) -> miette::Result<&MappingSource> {
        fn build_pypi_name_mapping_source(
            manifest: &WorkspaceManifest,
            channel_config: &ChannelConfig,
        ) -> miette::Result<MappingSource> {
            match manifest.workspace.conda_pypi_map.clone() {
                Some(map) => {
                    let channel_to_location_map = map
                        .into_iter()
                        .map(|(key, value)| {
                            let key = key.into_channel(channel_config).into_diagnostic()?;
                            Ok((key, value))
                        })
                        .collect::<miette::Result<HashMap<Channel, String>>>()?;

                    // User can disable the mapping by providing an empty map
                    if channel_to_location_map.is_empty() {
                        return Ok(MappingSource::Disabled);
                    }

                    let project_channels: HashSet<_> = manifest
                        .workspace
                        .channels
                        .iter()
                        .map(|pc| pc.channel.clone().into_channel(channel_config))
                        .try_collect()
                        .into_diagnostic()?;

                    let feature_channels: HashSet<_> = manifest
                        .features
                        .values()
                        .flat_map(|feature| feature.channels.iter())
                        .flatten()
                        .map(|pc| pc.channel.clone().into_channel(channel_config))
                        .try_collect()
                        .into_diagnostic()?;

                    let project_and_feature_channels: HashSet<_> =
                        project_channels.union(&feature_channels).collect();

                    for channel in channel_to_location_map.keys() {
                        if !project_and_feature_channels.contains(channel) {
                            let channels = project_and_feature_channels
                                .iter()
                                .map(|c| c.name.clone().unwrap_or_else(|| c.base_url.to_string()))
                                .sorted()
                                .collect::<Vec<_>>()
                                .join(", ");
                            miette::bail!(
                                "conda-pypi-map is defined: the {} is missing from the channels array, which currently are: {}",
                                console::style(
                                    channel
                                        .name
                                        .clone()
                                        .unwrap_or_else(|| channel.base_url.to_string())
                                )
                                .bold(),
                                channels
                            );
                        }
                    }

                    let mapping = channel_to_location_map
                        .iter()
                        .map(|(channel, mapping_location)| {
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

                            Ok((channel.canonical_name().trim_end_matches('/').into(), url_or_path))
                        })
                        .collect::<miette::Result<HashMap<ChannelName, MappingLocation>>>()?;

                    Ok(MappingSource::Custom(CustomMapping::new(mapping).into()))
                }
                None => Ok(MappingSource::Prefix),
            }
        }
        self.mapping_source.get_or_try_init(|| {
            build_pypi_name_mapping_source(&self.workspace.value, &self.channel_config())
        })
    }

    /// Constructs a new lock-file where some of the constraints have been
    /// removed.
    fn unlock_packages(
        &self,
        lock_file: &LockFile,
        conda_packages: HashSet<PackageName>,
        pypi_packages: HashSet<pep508_rs::PackageName>,
        affected_environments: HashSet<(&str, Platform)>,
    ) -> LockFile {
        filter_lock_file(self, lock_file, |env, platform, package| {
            if affected_environments.contains(&(env.name().as_str(), platform)) {
                match package {
                    LockedPackageRef::Conda(package) => {
                        !conda_packages.contains(&package.record().name)
                    }
                    LockedPackageRef::Pypi(package, _env) => !pypi_packages.contains(&package.name),
                }
            } else {
                true
            }
        })
    }
}

pub struct UpdateDeps {
    pub implicit_constraints: HashMap<String, String>,
    pub lock_file_diff: LockFileDiff,
}

impl<'source> HasWorkspaceManifest<'source> for &'source Workspace {
    fn workspace_manifest(&self) -> &'source WorkspaceManifest {
        &self.workspace.value
    }
}

/// Get or initialize the activated environment variables
pub async fn get_activated_environment_variables<'a>(
    project_env_vars: &'a HashMap<EnvironmentName, EnvironmentVars>,
    environment: &Environment<'_>,
    current_env_var_behavior: CurrentEnvVarBehavior,
    lock_file: Option<&LockFile>,
    force_activate: bool,
    experimental_cache: bool,
) -> miette::Result<&'a HashMap<String, String>> {
    let vars = project_env_vars.get(environment.name()).ok_or_else(|| {
        miette::miette!(
            "{} environment should be already created during project creation",
            environment.name()
        )
    })?;
    match current_env_var_behavior {
        CurrentEnvVarBehavior::Clean => {
            vars.clean()
                .get_or_try_init(async {
                    initialize_env_variables(
                        environment,
                        current_env_var_behavior,
                        lock_file,
                        force_activate,
                        experimental_cache,
                    )
                    .await
                })
                .await
        }
        CurrentEnvVarBehavior::Exclude => {
            vars.pixi_only()
                .get_or_try_init(async {
                    initialize_env_variables(
                        environment,
                        current_env_var_behavior,
                        lock_file,
                        force_activate,
                        experimental_cache,
                    )
                    .await
                })
                .await
        }
        CurrentEnvVarBehavior::Include => {
            vars.full()
                .get_or_try_init(async {
                    initialize_env_variables(
                        environment,
                        current_env_var_behavior,
                        lock_file,
                        force_activate,
                        experimental_cache,
                    )
                    .await
                })
                .await
        }
    }
}

/// Create a symlink from the directory to the custom target directory
#[cfg(not(windows))]
fn create_symlink(target_dir: &Path, symlink_dir: &Path) {
    if symlink_dir.exists() {
        tracing::debug!(
            "Symlink already exists at '{}', skipping creating symlink.",
            symlink_dir.display()
        );
        return;
    }
    let parent = symlink_dir
        .parent()
        .expect("symlink dir should have parent");
    fs_extra::dir::create_all(parent, false)
        .map_err(|e| tracing::error!("Failed to create directory '{}': {}", parent.display(), e))
        .ok();

    symlink(target_dir, symlink_dir)
        .map_err(|e| {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                tracing::error!(
                    "Failed to create symlink from '{}' to '{}': {}",
                    target_dir.display(),
                    symlink_dir.display(),
                    e
                )
            }
        })
        .ok();
}

/// Write a warning file to the default pixi directory to inform the user that
/// symlinks are not supported on this platform (Windows).
#[cfg(windows)]
fn write_warning_file(default_envs_dir: &PathBuf, envs_dir_name: &Path) {
    let warning_file = default_envs_dir.join("README.txt");
    if warning_file.exists() {
        tracing::debug!(
            "Symlink warning file already exists at '{}', skipping writing warning file.",
            warning_file.display()
        );
        return;
    }
    let warning_message = format!(
        "Environments are installed in a custom detached-environments directory: {}.\n\
        Symlinks are not supported on this platform so environments will not be reachable from the default ('.pixi/envs') directory.",
        envs_dir_name.display()
    );

    // Create directory if it doesn't exist
    if let Err(e) = fs_err::create_dir_all(default_envs_dir) {
        tracing::error!(
            "Failed to create directory '{}': {}",
            default_envs_dir.display(),
            e
        );
        return;
    }

    // Write warning message to file
    match fs_err::write(&warning_file, warning_message.clone()) {
        Ok(_) => tracing::info!(
            "Symlink warning file written to '{}': {}",
            warning_file.display(),
            warning_message
        ),
        Err(e) => tracing::error!(
            "Failed to write symlink warning file to '{}': {}",
            warning_file.display(),
            e
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use insta::{assert_debug_snapshot, assert_snapshot};
    use itertools::Itertools;
    use pixi_manifest::{FeatureName, FeaturesExt};
    use rattler_conda_types::{Platform, Version};
    use rattler_virtual_packages::{LibC, VirtualPackage};

    use super::*;

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = ["linux-64", "win-64"]
        "#;

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

            let workspace = Workspace::from_str(Path::new("pixi.toml"), &file_content).unwrap();
            let expected_result = vec![VirtualPackage::LibC(LibC {
                family: "glibc".to_string(),
                version: Version::from_str("2.12").unwrap(),
            })];

            let virtual_packages = workspace
                .default_environment()
                .system_requirements()
                .virtual_packages();

            assert_eq!(virtual_packages, expected_result);
        }
    }

    fn format_dependencies(deps: pixi_manifest::CondaDependencies) -> String {
        deps.iter_specs()
            .map(|(name, spec)| format!("{} = {}", name.as_source(), spec.to_toml_value()))
            .join("\n")
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

        let workspace = Workspace::from_str(
            Path::new("pixi.toml"),
            format!("{PROJECT_BOILERPLATE}\n{file_contents}").as_str(),
        )
        .unwrap();

        assert_snapshot!(format_dependencies(
            workspace
                .default_environment()
                .combined_dependencies(Some(Platform::Linux64))
        ));
    }

    #[test]
    #[ignore]
    fn test_dependency_set_with_build_section() {
        let file_contents = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = ["linux-64", "win-64"]
        preview = ["pixi-build"]
        [dependencies]
        foo = "1.0"

        [package]

        [build-system]
        channels = []
        dependencies = []
        build-backend = "foobar"

        [host-dependencies]
        libc = "2.12"

        [build-dependencies]
        bar = "1.0"
        "#;

        let workspace = Workspace::from_str(
            Path::new("pixi.toml"),
            format!("{PROJECT_BOILERPLATE}\n{file_contents}").as_str(),
        )
        .unwrap();

        assert_snapshot!(format_dependencies(
            workspace
                .default_environment()
                .combined_dependencies(Some(Platform::Linux64))
        ));
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
        let workspace = Workspace::from_str(
            Path::new("pixi.toml"),
            format!("{PROJECT_BOILERPLATE}\n{file_contents}").as_str(),
        )
        .unwrap();

        assert_snapshot!(format_dependencies(
            workspace
                .default_environment()
                .combined_dependencies(Some(Platform::Linux64))
        ));
    }

    #[test]
    fn test_activation_scripts() {
        fn fmt_activation_scripts(scripts: Vec<String>) -> String {
            scripts.iter().join("\n")
        }

        // Using known files in the project so the test succeed including the file
        // check.
        let file_contents = r#"
            [target.linux-64.activation]
            scripts = ["Cargo.toml"]

            [target.win-64.activation]
            scripts = ["Cargo.lock"]

            [activation]
            scripts = ["pixi.toml", "pixi.lock"]
            "#;
        let workspace = Workspace::from_str(
            Path::new("pixi.toml"),
            format!("{PROJECT_BOILERPLATE}\n{file_contents}").as_str(),
        )
        .unwrap();

        assert_snapshot!(format!(
            "= Linux64\n{}\n\n= Win64\n{}\n\n= OsxArm64\n{}",
            fmt_activation_scripts(
                workspace
                    .default_environment()
                    .activation_scripts(Some(Platform::Linux64))
            ),
            fmt_activation_scripts(
                workspace
                    .default_environment()
                    .activation_scripts(Some(Platform::Win64))
            ),
            fmt_activation_scripts(
                workspace
                    .default_environment()
                    .activation_scripts(Some(Platform::OsxArm64))
            )
        ));
    }

    #[test]
    fn test_target_specific_tasks() {
        // Using known files in the project so the test succeed including the file
        // check.
        let file_contents = r#"
            [tasks]
            test = "test multi"

            [target.win-64.tasks]
            test = "test win"

            [target.linux-64.tasks]
            test = "test linux"
            "#;
        let workspace = Workspace::from_str(
            Path::new("pixi.toml"),
            format!("{PROJECT_BOILERPLATE}\n{file_contents}").as_str(),
        )
        .unwrap();

        assert_debug_snapshot!(workspace
            .workspace
            .value
            .tasks(Some(Platform::Osx64), &FeatureName::Default)
            .unwrap());
        assert_debug_snapshot!(workspace
            .workspace
            .value
            .tasks(Some(Platform::Win64), &FeatureName::Default)
            .unwrap());
        assert_debug_snapshot!(workspace
            .workspace
            .value
            .tasks(Some(Platform::Linux64), &FeatureName::Default)
            .unwrap());
    }

    #[test]
    fn test_mapping_location() {
        let file_contents = r#"
            [workspace]
            name = "foo"
            channels = ["conda-forge", "pytorch"]
            platforms = []
            conda-pypi-map = {conda-forge = "https://github.com/prefix-dev/parselmouth/blob/main/files/compressed_mapping.json", pytorch = ""}
            "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        let mapping = workspace.pypi_name_mapping_source().unwrap();
        let channel = Channel::from_str("conda-forge", &workspace.channel_config()).unwrap();
        let canonical_name = channel.canonical_name();

        let canonical_channel_name = canonical_name.trim_end_matches('/');

        assert_eq!(mapping.custom().unwrap().mapping.get(canonical_channel_name).unwrap(), &MappingLocation::Url(Url::parse("https://github.com/prefix-dev/parselmouth/blob/main/files/compressed_mapping.json").unwrap()));

        // Check url channel as map key
        let file_contents = r#"
            [workspace]
            name = "foo"
            channels = ["https://prefix.dev/test-channel"]
            platforms = []
            conda-pypi-map = {"https://prefix.dev/test-channel" = "mapping.json"}
            "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        let mapping = workspace.pypi_name_mapping_source().unwrap();
        assert_eq!(
            mapping
                .custom()
                .unwrap()
                .mapping
                .get(
                    Channel::from_str(
                        "https://prefix.dev/test-channel",
                        &workspace.channel_config()
                    )
                    .unwrap()
                    .canonical_name()
                    .trim_end_matches('/')
                )
                .unwrap(),
            &MappingLocation::Path(PathBuf::from("mapping.json"))
        );
    }

    #[test]
    fn test_mapping_ensure_feature_channels_also_checked() {
        let file_contents = r#"
            [workspace]
            name = "foo"
            channels = ["conda-forge", "pytorch"]
            platforms = []
            conda-pypi-map = {custom-feature-channel = "https://github.com/prefix-dev/parselmouth/blob/main/files/compressed_mapping.json"}

            [feature.a]
            channels = ["custom-feature-channel"]
            "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        assert!(workspace.pypi_name_mapping_source().is_ok());

        let non_existing_channel = r#"
            [workspace]
            name = "foo"
            channels = ["conda-forge", "pytorch"]
            platforms = []
            conda-pypi-map = {non-existing-channel = "https://github.com/prefix-dev/parselmouth/blob/main/files/compressed_mapping.json"}
            "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), non_existing_channel).unwrap();

        // We output error message with bold channel name,
        // so we need to disable colors for snapshot
        console::set_colors_enabled(false);

        insta::assert_snapshot!(workspace.pypi_name_mapping_source().unwrap_err());
    }
}
