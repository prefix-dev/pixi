mod environment;
pub mod errors;
pub mod grouped_environment;
mod has_project_ref;
mod repodata;
mod solve_group;
pub mod virtual_packages;

#[cfg(not(windows))]
use std::os::unix::fs::symlink;
use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet},
    fmt::{Debug, Formatter},
    hash::Hash,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use async_once_cell::OnceCell as AsyncCell;
pub use environment::Environment;
pub use has_project_ref::HasProjectRef;
use indexmap::Equivalent;
use itertools::Itertools;
use miette::IntoDiagnostic;
use once_cell::sync::OnceCell;
use pixi_config::Config;
use pixi_consts::consts;
use pixi_manifest::{
    EnvironmentName, Environments, HasManifestRef, Manifest, ParsedManifest, SpecType,
};
use pixi_utils::reqwest::build_reqwest_clients;
use pypi_mapping::{ChannelName, CustomMapping, MappingLocation, MappingSource};
use rattler_conda_types::{Channel, ChannelConfig, Version};
use rattler_repodata_gateway::Gateway;
use reqwest_middleware::ClientWithMiddleware;
pub use solve_group::SolveGroup;
use url::{ParseError, Url};
use xxhash_rust::xxh3::xxh3_64;

use crate::activation::{initialize_env_variables, CurrentEnvVarBehavior};

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

/// The pixi project, this main struct to interact with the project. This struct
/// holds the `Manifest` and has functions to modify or request information from
/// it. This allows in the future to have multiple environments or manifests
/// linked to a project.
#[derive(Clone)]
pub struct Project {
    /// Root folder of the project
    root: PathBuf,
    /// Reqwest client shared for this project.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    client: OnceLock<(reqwest::Client, ClientWithMiddleware)>,
    /// The repodata gateway to use for answering queries about repodata.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    repodata_gateway: OnceLock<Gateway>,
    /// The manifest for the project
    pub(crate) manifest: Manifest,
    /// The environment variables that are activated when the environment is
    /// activated. Cached per environment, for both clean and normal
    env_vars: HashMap<EnvironmentName, EnvironmentVars>,
    /// The cache that contains mapping
    mapping_source: OnceCell<MappingSource>,
    /// The global configuration as loaded from the config file(s)
    config: Config,
}

impl Debug for Project {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project")
            .field("root", &self.root)
            .field("manifest", &self.manifest)
            .finish()
    }
}

impl Borrow<ParsedManifest> for Project {
    fn borrow(&self) -> &ParsedManifest {
        self.manifest.borrow()
    }
}

impl Project {
    /// Constructs a new instance from an internal manifest representation
    pub(crate) fn from_manifest(manifest: Manifest) -> Self {
        let env_vars = Project::init_env_vars(&manifest.parsed.environments);

        let root = manifest
            .path
            .parent()
            .expect("manifest path should always have a parent")
            .to_owned();

        let config = Config::load(&root);

        Self {
            root,
            client: Default::default(),
            manifest,
            env_vars,
            mapping_source: Default::default(),
            config,
            repodata_gateway: Default::default(),
        }
    }

    /// Initialize empty map of environments variables
    fn init_env_vars(environments: &Environments) -> HashMap<EnvironmentName, EnvironmentVars> {
        environments
            .iter()
            .map(|environment| (environment.name.clone(), EnvironmentVars::new()))
            .collect()
    }

    /// Constructs a project from a manifest.
    pub fn from_str(manifest_path: &Path, content: &str) -> miette::Result<Self> {
        let manifest = Manifest::from_str(manifest_path, content)?;
        Ok(Self::from_manifest(manifest))
    }

    /// Discovers the project manifest file in the current directory or any of
    /// the parent directories, or use the manifest specified by the
    /// environment. This will also set the current working directory to the
    /// project root.
    pub(crate) fn discover() -> miette::Result<Self> {
        let project_toml = find_project_manifest(std::env::current_dir().into_diagnostic()?);

        if std::env::var("PIXI_IN_SHELL").is_ok() {
            if let Ok(env_manifest_path) = std::env::var("PIXI_PROJECT_MANIFEST") {
                if let Some(project_toml) = project_toml {
                    if env_manifest_path != project_toml.to_string_lossy() {
                        tracing::warn!(
                            "Using manifest {} from `PIXI_PROJECT_MANIFEST` rather than local {}",
                            env_manifest_path,
                            project_toml.to_string_lossy()
                        );
                    }
                }
                return Self::from_path(Path::new(env_manifest_path.as_str()));
            }
        }

        let project_toml = match project_toml {
            Some(file) => file,
            None => miette::bail!(
                "could not find {} or {} which is configured to use pixi",
                consts::PROJECT_MANIFEST,
                consts::PYPROJECT_MANIFEST
            ),
        };

        Self::from_path(&project_toml)
    }

    /// Loads a project from manifest file.
    pub fn from_path(manifest_path: &Path) -> miette::Result<Self> {
        let manifest = Manifest::from_path(manifest_path)?;
        Ok(Project::from_manifest(manifest))
    }

    /// Loads a project manifest file or discovers it in the current directory
    /// or any of the parent
    pub fn load_or_else_discover(manifest_path: Option<&Path>) -> miette::Result<Self> {
        let project = match manifest_path {
            Some(path) => Project::from_path(path)?,
            None => Project::discover()?,
        };
        Ok(project)
    }

    /// Warns if Pixi is using a manifest from an environment variable rather
    /// than a discovered version
    pub(crate) fn warn_on_discovered_from_env(manifest_path: Option<&Path>) {
        if manifest_path.is_none() && std::env::var("PIXI_IN_SHELL").is_ok() {
            if let Ok(current_dir) = std::env::current_dir() {
                let discover_path = find_project_manifest(current_dir);
                let env_path = std::env::var("PIXI_PROJECT_MANIFEST");

                if let (Some(discover_path), Ok(env_path)) = (discover_path, env_path) {
                    if env_path.as_str() != discover_path.to_str().unwrap() {
                        tracing::warn!(
                            "Used manifest {} from `PIXI_PROJECT_MANIFEST` rather than local {}",
                            env_path,
                            discover_path.to_string_lossy()
                        );
                    }
                }
            }
        }
    }

    pub(crate) fn with_cli_config<C>(mut self, config: C) -> Self
    where
        C: Into<Config>,
    {
        self.config = self.config.merge_config(config.into());
        self
    }

    /// Returns the name of the project
    pub fn name(&self) -> &str {
        self.manifest
            .parsed
            .project
            .name
            .as_ref()
            .expect("name should always be defined.")
    }

    /// Returns the version of the project
    pub fn version(&self) -> &Option<Version> {
        &self.manifest.parsed.project.version
    }

    /// Returns the description of the project
    pub(crate) fn description(&self) -> &Option<String> {
        &self.manifest.parsed.project.description
    }

    /// Returns the root directory of the project
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the pixi directory of the project [consts::PIXI_DIR]
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
                #[cfg(not(windows))]
                if default_envs_dir.exists() && !default_envs_dir.is_symlink() {
                    tracing::warn!(
                        "Environments found in '{}', this will be ignored and the environment will be installed in the 'detached-environments' directory: '{}'. It's advised to remove the {} folder from the default directory to avoid confusion{}.",
                        default_envs_dir.display(),
                        detached_environments_path.parent().expect("path should have parent").display(),
                        format!("{}/{}", consts::PIXI_DIR, consts::ENVIRONMENTS_DIR),
                        if cfg!(windows) { "" } else { " as a symlink can be made, please re-install after removal." }
                    );
                } else {
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

    /// Returns the path to the manifest file.
    pub(crate) fn manifest_path(&self) -> PathBuf {
        self.manifest.path.clone()
    }

    /// Returns the path to the lock file of the project
    /// [consts::PROJECT_LOCK_FILE]
    pub(crate) fn lock_file_path(&self) -> PathBuf {
        self.root.join(consts::PROJECT_LOCK_FILE)
    }

    /// Save back changes
    pub(crate) fn save(&mut self) -> miette::Result<()> {
        self.manifest.save()
    }

    /// Returns the default environment of the project.
    pub fn default_environment(&self) -> Environment<'_> {
        Environment::new(self, self.manifest.default_environment())
    }

    /// Returns the environment with the given name or `None` if no such
    /// environment exists.
    pub fn environment<Q>(&self, name: &Q) -> Option<Environment<'_>>
    where
        Q: ?Sized + Hash + Equivalent<EnvironmentName>,
    {
        Some(Environment::new(self, self.manifest.environment(name)?))
    }

    /// Returns the environments in this project.
    pub(crate) fn environments(&self) -> Vec<Environment> {
        self.manifest
            .parsed
            .environments
            .iter()
            .map(|env| Environment::new(self, env))
            .collect()
    }

    /// Returns an environment in this project based on a name or an environment
    /// variable.
    pub(crate) fn environment_from_name_or_env_var(
        &self,
        name: Option<String>,
    ) -> miette::Result<Environment> {
        let environment_name = EnvironmentName::from_arg_or_env_var(name).into_diagnostic()?;
        self.environment(&environment_name)
            .ok_or_else(|| miette::miette!("unknown environment '{environment_name}'"))
    }

    /// Get or initialize the activated environment variables
    pub async fn get_activated_environment_variables(
        &self,
        environment: &Environment<'_>,
        current_env_var_behavior: CurrentEnvVarBehavior,
    ) -> miette::Result<&HashMap<String, String>> {
        let vars = self.env_vars.get(environment.name()).ok_or_else(|| {
            miette::miette!(
                "{} environment should be already created during project creation",
                environment.name()
            )
        })?;
        match current_env_var_behavior {
            CurrentEnvVarBehavior::Clean => {
                vars.clean()
                    .get_or_try_init(async {
                        initialize_env_variables(environment, current_env_var_behavior).await
                    })
                    .await
            }
            CurrentEnvVarBehavior::Exclude => {
                vars.pixi_only()
                    .get_or_try_init(async {
                        initialize_env_variables(environment, current_env_var_behavior).await
                    })
                    .await
            }
            CurrentEnvVarBehavior::Include => {
                vars.full()
                    .get_or_try_init(async {
                        initialize_env_variables(environment, current_env_var_behavior).await
                    })
                    .await
            }
        }
    }
    /// Returns all the solve groups in the project.
    pub(crate) fn solve_groups(&self) -> Vec<SolveGroup> {
        self.manifest
            .parsed
            .solve_groups
            .iter()
            .map(|group| SolveGroup {
                project: self,
                solve_group: group,
            })
            .collect()
    }

    /// Returns the solve group with the given name or `None` if no such group
    /// exists.
    pub(crate) fn solve_group(&self, name: &str) -> Option<SolveGroup> {
        self.manifest
            .parsed
            .solve_groups
            .find(name)
            .map(|group| SolveGroup {
                project: self,
                solve_group: group,
            })
    }

    /// Returns the reqwest client used for http networking
    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client_and_authenticated_client().0
    }

    /// Create an authenticated reqwest client for this project
    /// use authentication from `rattler_networking`
    pub fn authenticated_client(&self) -> &ClientWithMiddleware {
        &self.client_and_authenticated_client().1
    }

    fn client_and_authenticated_client(&self) -> &(reqwest::Client, ClientWithMiddleware) {
        self.client
            .get_or_init(|| build_reqwest_clients(Some(&self.config)))
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    /// Construct a [`ChannelConfig`] that is specific to this project. This
    /// ensures that the root directory is set correctly.
    pub(crate) fn channel_config(&self) -> ChannelConfig {
        ChannelConfig {
            root_dir: self.root.clone(),
            ..self.config.global_channel_config().clone()
        }
    }

    pub(crate) fn task_cache_folder(&self) -> PathBuf {
        self.pixi_dir().join(consts::TASK_CACHE_DIR)
    }

    /// Returns what pypi mapping configuration we should use.
    /// It can be a custom one  in following format : conda_name: pypi_name
    /// Or we can use our self-hosted
    pub fn pypi_name_mapping_source(&self) -> miette::Result<&MappingSource> {
        fn build_pypi_name_mapping_source(
            manifest: &Manifest,
            channel_config: &ChannelConfig,
        ) -> miette::Result<MappingSource> {
            match manifest.parsed.project.conda_pypi_map.clone() {
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
                        .parsed
                        .project
                        .channels
                        .iter()
                        .map(|pc| pc.channel.clone().into_channel(channel_config))
                        .try_collect()
                        .into_diagnostic()?;

                    let feature_channels: HashSet<_> = manifest
                        .parsed
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
            build_pypi_name_mapping_source(&self.manifest, &self.channel_config())
        })
    }

    /// Returns the manifest of the project
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}

impl<'source> HasManifestRef<'source> for &'source Project {
    fn manifest(&self) -> &'source Manifest {
        Project::manifest(self)
    }
}

/// Iterates over the current directory and all its parent directories and
/// returns the manifest path in the first directory path that contains the
/// [`consts::PROJECT_MANIFEST`] or [`consts::PYPROJECT_MANIFEST`].
pub(crate) fn find_project_manifest(current_dir: PathBuf) -> Option<PathBuf> {
    let manifests = [consts::PROJECT_MANIFEST, consts::PYPROJECT_MANIFEST];

    for dir in current_dir.ancestors() {
        for manifest in &manifests {
            let path = dir.join(manifest);
            if !path.is_file() {
                continue;
            }

            match *manifest {
                consts::PROJECT_MANIFEST => return Some(path),
                consts::PYPROJECT_MANIFEST => {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if content.contains("[tool.pixi") {
                            return Some(path);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    None
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
    if let Err(e) = std::fs::create_dir_all(default_envs_dir) {
        tracing::error!(
            "Failed to create directory '{}': {}",
            default_envs_dir.display(),
            e
        );
        return;
    }

    // Write warning message to file
    match std::fs::write(&warning_file, warning_message.clone()) {
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
    use std::fs::File;
    use std::io::Write;
    use std::str::FromStr;

    use super::*;
    use insta::{assert_debug_snapshot, assert_snapshot};
    use itertools::Itertools;
    use pixi_manifest::{FeatureName, FeaturesExt};
    use rattler_conda_types::Platform;
    use rattler_virtual_packages::{LibC, VirtualPackage};
    use tempfile::tempdir;

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

            let manifest = Manifest::from_str(Path::new("pixi.toml"), &file_content).unwrap();
            let project = Project::from_manifest(manifest);
            let expected_result = vec![VirtualPackage::LibC(LibC {
                family: "glibc".to_string(),
                version: Version::from_str("2.12").unwrap(),
            })];

            let virtual_packages = project
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

        let manifest = Manifest::from_str(
            Path::new("pixi.toml"),
            format!("{PROJECT_BOILERPLATE}\n{file_contents}").as_str(),
        )
        .unwrap();
        let project = Project::from_manifest(manifest);

        assert_snapshot!(format_dependencies(
            project
                .default_environment()
                .dependencies(None, Some(Platform::Linux64))
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
        let manifest = Manifest::from_str(
            Path::new("pixi.toml"),
            format!("{PROJECT_BOILERPLATE}\n{file_contents}").as_str(),
        )
        .unwrap();
        let project = Project::from_manifest(manifest);

        assert_snapshot!(format_dependencies(
            project
                .default_environment()
                .dependencies(None, Some(Platform::Linux64))
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
        let manifest = Manifest::from_str(
            Path::new("pixi.toml"),
            format!("{PROJECT_BOILERPLATE}\n{file_contents}").as_str(),
        )
        .unwrap();
        let project = Project::from_manifest(manifest);

        assert_snapshot!(format!(
            "= Linux64\n{}\n\n= Win64\n{}\n\n= OsxArm64\n{}",
            fmt_activation_scripts(
                project
                    .default_environment()
                    .activation_scripts(Some(Platform::Linux64))
            ),
            fmt_activation_scripts(
                project
                    .default_environment()
                    .activation_scripts(Some(Platform::Win64))
            ),
            fmt_activation_scripts(
                project
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
        let manifest = Manifest::from_str(
            Path::new("pixi.toml"),
            format!("{PROJECT_BOILERPLATE}\n{file_contents}").as_str(),
        )
        .unwrap();

        let project = Project::from_manifest(manifest);

        assert_debug_snapshot!(project
            .manifest
            .tasks(Some(Platform::Osx64), &FeatureName::Default)
            .unwrap());
        assert_debug_snapshot!(project
            .manifest
            .tasks(Some(Platform::Win64), &FeatureName::Default)
            .unwrap());
        assert_debug_snapshot!(project
            .manifest
            .tasks(Some(Platform::Linux64), &FeatureName::Default)
            .unwrap());
    }

    #[test]
    fn test_mapping_location() {
        let file_contents = r#"
            [project]
            name = "foo"
            channels = ["conda-forge", "pytorch"]
            platforms = []
            conda-pypi-map = {conda-forge = "https://github.com/prefix-dev/parselmouth/blob/main/files/compressed_mapping.json", pytorch = ""}
            "#;
        let manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();
        let project = Project::from_manifest(manifest);

        let mapping = project.pypi_name_mapping_source().unwrap();
        let channel = Channel::from_str("conda-forge", &project.channel_config()).unwrap();
        let canonical_name = channel.canonical_name();

        let canonical_channel_name = canonical_name.trim_end_matches('/');

        assert_eq!(mapping.custom().unwrap().mapping.get(canonical_channel_name).unwrap(), &MappingLocation::Url(Url::parse("https://github.com/prefix-dev/parselmouth/blob/main/files/compressed_mapping.json").unwrap()));

        // Check url channel as map key
        let file_contents = r#"
            [project]
            name = "foo"
            channels = ["https://prefix.dev/test-channel"]
            platforms = []
            conda-pypi-map = {"https://prefix.dev/test-channel" = "mapping.json"}
            "#;
        let manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();
        let project = Project::from_manifest(manifest);

        let mapping = project.pypi_name_mapping_source().unwrap();
        assert_eq!(
            mapping
                .custom()
                .unwrap()
                .mapping
                .get(
                    Channel::from_str("https://prefix.dev/test-channel", &project.channel_config())
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
            [project]
            name = "foo"
            channels = ["conda-forge", "pytorch"]
            platforms = []
            conda-pypi-map = {custom-feature-channel = "https://github.com/prefix-dev/parselmouth/blob/main/files/compressed_mapping.json"}

            [feature.a]
            channels = ["custom-feature-channel"]
            "#;
        let manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();
        let project = Project::from_manifest(manifest);

        assert!(project.pypi_name_mapping_source().is_ok());

        let non_existing_channel = r#"
            [project]
            name = "foo"
            channels = ["conda-forge", "pytorch"]
            platforms = []
            conda-pypi-map = {non-existing-channel = "https://github.com/prefix-dev/parselmouth/blob/main/files/compressed_mapping.json"}
            "#;
        let manifest = Manifest::from_str(Path::new("pixi.toml"), non_existing_channel).unwrap();
        let project = Project::from_manifest(manifest);

        // We output error message with bold channel name,
        // so we need to disable colors for snapshot
        console::set_colors_enabled(false);

        insta::assert_snapshot!(project.pypi_name_mapping_source().unwrap_err());
    }

    #[test]
    fn test_find_project_manifest_in_current_dir() {
        for manifest in &[consts::PROJECT_MANIFEST, consts::PYPROJECT_MANIFEST] {
            let dir = tempdir().unwrap();
            let project_manifest_path = dir.path().join(manifest);

            // Create manifest
            let mut file = File::create(&project_manifest_path).unwrap();
            writeln!(file, "[project]").unwrap();
            if manifest == &consts::PYPROJECT_MANIFEST {
                writeln!(file, "[tool.pixi.project]").unwrap();
            }

            assert_eq!(
                find_project_manifest(dir.into_path()),
                Some(project_manifest_path)
            );
        }
    }

    #[test]
    fn test_find_project_manifest_with_multiple() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join(consts::PROJECT_MANIFEST);
        let pyproject_manifest_path = dir.path().join(consts::PYPROJECT_MANIFEST);

        // Create manifests
        let mut file = File::create(&manifest_path).unwrap();
        writeln!(file, "[project]").unwrap();
        let mut file = File::create(&pyproject_manifest_path).unwrap();
        writeln!(file, "[project]").unwrap();
        writeln!(file, "[tool.pixi.project]").unwrap();

        assert_eq!(find_project_manifest(dir.into_path()), Some(manifest_path));
    }

    #[test]
    fn test_find_manifest_closest_to_current_dir() {
        // Create a file structure like:
        // root
        // ├── child
        // │   └── pyproject.toml
        // ├── non-pixi-child
        // │   └── pyproject.toml
        // └── pixi.toml
        //
        // And verify that the correct manifest is found in each directory

        let dir = tempdir().unwrap();
        let pixi_child_dir = dir.path().join("child");
        let non_pixi_child_dir = dir.path().join("non-pixi-child");

        let manifest_path_root = dir.path().join(consts::PROJECT_MANIFEST);
        let manifest_path_pixi_child = pixi_child_dir.join(consts::PYPROJECT_MANIFEST);
        let manifest_path_non_pixi_child = non_pixi_child_dir.join(consts::PYPROJECT_MANIFEST);

        // Create manifests
        // Root manifest is normal pixi.toml
        let mut file = File::create(&manifest_path_root).unwrap();
        writeln!(file, "[project]").unwrap();

        // Pixi child manifest is pyproject.toml with pixi tool
        std::fs::create_dir_all(&pixi_child_dir).unwrap();
        let mut file = File::create(&manifest_path_pixi_child).unwrap();
        writeln!(file, "[project]").unwrap();
        writeln!(file, "[tool.pixi.project]").unwrap();

        // Non pixi child manifest is pyproject.toml without pixi tool
        std::fs::create_dir_all(&non_pixi_child_dir).unwrap();
        let mut file = File::create(&manifest_path_non_pixi_child).unwrap();
        writeln!(file, "[project]").unwrap();

        // In root use root manifest
        assert_eq!(
            find_project_manifest(dir.into_path()),
            Some(manifest_path_root.clone())
        );

        // In pixi child use pixi child manifest
        assert_eq!(
            find_project_manifest(pixi_child_dir),
            Some(manifest_path_pixi_child)
        );

        // In non pixi child use root manifest
        assert_eq!(
            find_project_manifest(non_pixi_child_dir),
            Some(manifest_path_root)
        );
    }
}
