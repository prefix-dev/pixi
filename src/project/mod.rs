mod dependencies;
mod environment;
pub mod errors;
pub mod grouped_environment;
pub mod has_features;
pub mod manifest;
mod repodata;
mod solve_group;
pub mod virtual_packages;

use async_once_cell::OnceCell as AsyncCell;
use indexmap::Equivalent;
use miette::{IntoDiagnostic, NamedSource};
use once_cell::sync::OnceCell;

use rattler_conda_types::Version;
use reqwest_middleware::ClientWithMiddleware;
use std::hash::Hash;

use rattler_repodata_gateway::Gateway;

#[cfg(not(windows))]
use std::os::unix::fs::symlink;

use std::sync::OnceLock;
use std::{
    collections::{HashMap, HashSet},
    env,
    fmt::{Debug, Formatter},
    path::{Path, PathBuf},
    sync::Arc,
};
use xxhash_rust::xxh3::xxh3_64;

use crate::activation::{get_environment_variables, run_activation};
use crate::config::Config;
use crate::consts::{self, PROJECT_MANIFEST, PYPROJECT_MANIFEST};
use crate::project::grouped_environment::GroupedEnvironment;

use crate::pypi_mapping::MappingSource;
use crate::utils::reqwest::build_reqwest_clients;
use manifest::{EnvironmentName, Manifest};

use self::manifest::{pyproject::PyProjectToml, Environments};
pub use dependencies::{CondaDependencies, PyPiDependencies};
pub use environment::Environment;
pub use solve_group::SolveGroup;

static CUSTOM_TARGET_DIR_WARN: OnceCell<()> = OnceCell::new();

/// The dependency types we support
#[derive(Debug, Copy, Clone)]
pub enum DependencyType {
    CondaDependency(SpecType),
    PypiDependency,
}

impl DependencyType {
    /// Convert to a name used in the manifest
    pub fn name(&self) -> &'static str {
        match self {
            DependencyType::CondaDependency(dep) => dep.name(),
            DependencyType::PypiDependency => consts::PYPI_DEPENDENCIES,
        }
    }
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
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

    /// Returns all the variants of the enum
    pub fn all() -> impl Iterator<Item = SpecType> {
        [SpecType::Run, SpecType::Host, SpecType::Build].into_iter()
    }
}

/// The pixi project, this main struct to interact with the project. This struct holds the
/// `Manifest` and has functions to modify or request information from it.
/// This allows in the future to have multiple environments or manifests linked to a project.
#[derive(Clone)]
pub struct Project {
    /// Root folder of the project
    root: PathBuf,
    /// Reqwest client shared for this project
    client: reqwest::Client,
    /// Authenticated reqwest client shared for this project
    authenticated_client: ClientWithMiddleware,
    /// The repodata gateway to use for answering queries about repodata.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    repodata_gateway: OnceLock<Gateway>,
    /// The manifest for the project
    pub(crate) manifest: Manifest,
    /// The cache that contains environment variables
    env_vars: HashMap<EnvironmentName, Arc<AsyncCell<HashMap<String, String>>>>,
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

impl Project {
    /// Constructs a new instance from an internal manifest representation
    pub fn from_manifest(manifest: Manifest) -> Self {
        let env_vars = Project::init_env_vars(&manifest.parsed.environments);

        let root = manifest.path.parent().unwrap_or(Path::new("")).to_owned();

        let config = Config::load(&root);

        let (client, authenticated_client) = build_reqwest_clients(Some(&config));

        Self {
            root,
            client,
            authenticated_client,
            manifest,
            env_vars,
            mapping_source: Default::default(),
            config,
            repodata_gateway: Default::default(),
        }
    }

    //Initialize empty map of environments variables
    fn init_env_vars(
        environments: &Environments,
    ) -> HashMap<EnvironmentName, Arc<AsyncCell<HashMap<String, String>>>> {
        environments
            .iter()
            .map(|environment| (environment.name.clone(), Arc::new(AsyncCell::new())))
            .collect()
    }

    /// Constructs a project from a manifest.
    /// Assumes the manifest is a Pixi manifest
    pub fn from_str(manifest_path: &Path, content: &str) -> miette::Result<Self> {
        let manifest = Manifest::from_str(manifest_path, content)?;
        Ok(Self::from_manifest(manifest))
    }

    /// Discovers the project manifest file in the current directory or any of the parent
    /// directories, or use the manifest specified by the environment.
    /// This will also set the current working directory to the project root.
    pub fn discover() -> miette::Result<Self> {
        let project_toml = find_project_manifest();

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
                return Self::load(Path::new(env_manifest_path.as_str()));
            }
        }

        let project_toml = match project_toml {
            Some(file) => file,
            None => miette::bail!(
                "could not find {} or {} which is configured to use pixi",
                PROJECT_MANIFEST,
                PYPROJECT_MANIFEST
            ),
        };

        Self::load(&project_toml)
    }

    /// Returns the source code of the project as [`NamedSource`].
    /// Used in error reporting.
    pub fn manifest_named_source(&self) -> NamedSource<String> {
        NamedSource::new(self.manifest.file_name(), self.manifest.contents.clone())
    }

    /// Loads a project from manifest file.
    pub fn load(manifest_path: &Path) -> miette::Result<Self> {
        // Determine the parent directory of the manifest file
        let full_path = dunce::canonicalize(manifest_path).into_diagnostic()?;

        let root = full_path
            .parent()
            .ok_or_else(|| miette::miette!("can not find parent of {}", manifest_path.display()))?;

        // Load the TOML document
        let manifest = Manifest::from_path(manifest_path)?;

        let env_vars = Project::init_env_vars(&manifest.parsed.environments);

        // Load the user configuration from the local project and all default locations
        let config = Config::load(root);

        let (client, authenticated_client) = build_reqwest_clients(Some(&config));

        Ok(Self {
            root: root.to_owned(),
            client,
            authenticated_client,
            manifest,
            env_vars,
            mapping_source: Default::default(),
            config,
            repodata_gateway: Default::default(),
        })
    }

    /// Loads a project manifest file or discovers it in the current directory or any of the parent
    pub fn load_or_else_discover(manifest_path: Option<&Path>) -> miette::Result<Self> {
        let project = match manifest_path {
            Some(path) => Project::load(path)?,
            None => Project::discover()?,
        };
        Ok(project)
    }

    /// Warns if Pixi is using a manifest from an environment variable rather than a discovered version
    pub fn warn_on_discovered_from_env(manifest_path: Option<&Path>) {
        if manifest_path.is_none() && std::env::var("PIXI_IN_SHELL").is_ok() {
            let discover_path = find_project_manifest();
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

    pub fn with_cli_config<C>(mut self, config: C) -> Self
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
    pub fn description(&self) -> &Option<String> {
        &self.manifest.parsed.project.description
    }

    /// Returns the root directory of the project
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the pixi directory of the project [consts::PIXI_DIR]
    pub fn pixi_dir(&self) -> PathBuf {
        self.root.join(consts::PIXI_DIR)
    }

    /// Create the detached-environments path for this project if it is set in the config
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

    /// Returns the environment directory
    pub fn environments_dir(&self) -> PathBuf {
        let default_envs_dir = self.pixi_dir().join(consts::ENVIRONMENTS_DIR);

        // Early out if detached-environments is not set
        if self.config().detached_environments().is_false() {
            return default_envs_dir;
        }

        // If the detached-environments path is set, use it instead of the default directory.
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

    /// Returns the solve group environments directory
    pub fn solve_group_environments_dir(&self) -> PathBuf {
        // If the detached-environments path is set, use it instead of the default directory.
        if let Some(detached_environments_path) = self.detached_environments_path() {
            return detached_environments_path.join(consts::SOLVE_GROUP_ENVIRONMENTS_DIR);
        }
        self.pixi_dir().join(consts::SOLVE_GROUP_ENVIRONMENTS_DIR)
    }

    /// Returns the path to the manifest file.
    pub fn manifest_path(&self) -> PathBuf {
        self.manifest.path.clone()
    }

    /// Returns the path to the lock file of the project [consts::PROJECT_LOCK_FILE]
    pub fn lock_file_path(&self) -> PathBuf {
        self.root.join(consts::PROJECT_LOCK_FILE)
    }

    /// Save back changes
    pub fn save(&mut self) -> miette::Result<()> {
        self.manifest.save()
    }

    /// Returns the default environment of the project.
    pub fn default_environment(&self) -> Environment<'_> {
        Environment::new(self, self.manifest.default_environment())
    }

    /// Returns the environment with the given name or `None` if no such environment exists.
    pub fn environment<Q: ?Sized>(&self, name: &Q) -> Option<Environment<'_>>
    where
        Q: Hash + Equivalent<EnvironmentName>,
    {
        Some(Environment::new(self, self.manifest.environment(name)?))
    }

    /// Returns the environments in this project.
    pub fn environments(&self) -> Vec<Environment> {
        self.manifest
            .parsed
            .environments
            .iter()
            .map(|env| Environment::new(self, env))
            .collect()
    }

    /// Returns an environment in this project based on a name or an environment variable.
    pub fn environment_from_name_or_env_var(
        &self,
        name: Option<String>,
    ) -> miette::Result<Environment> {
        let environment_name = EnvironmentName::from_arg_or_env_var(name).into_diagnostic()?;
        self.environment(&environment_name)
            .ok_or_else(|| miette::miette!("unknown environment '{environment_name}'"))
    }

    /// Returns all the solve groups in the project.
    pub fn solve_groups(&self) -> Vec<SolveGroup> {
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

    /// Returns the solve group with the given name or `None` if no such group exists.
    pub fn solve_group(&self, name: &str) -> Option<SolveGroup> {
        self.manifest
            .parsed
            .solve_groups
            .find(name)
            .map(|group| SolveGroup {
                project: self,
                solve_group: group,
            })
    }

    /// Return the grouped environments, which are all solve-groups and the environments that need to be solved.
    pub fn grouped_environments(&self) -> Vec<GroupedEnvironment> {
        let mut environments = HashSet::new();
        environments.extend(
            self.environments()
                .into_iter()
                .filter(|env| env.solve_group().is_none())
                .map(GroupedEnvironment::from),
        );
        environments.extend(
            self.solve_groups()
                .into_iter()
                .map(GroupedEnvironment::from),
        );
        environments.into_iter().collect()
    }

    /// Returns true if the project contains any reference pypi dependencies. Even if just
    /// `[pypi-dependencies]` is specified without any requirements this will return true.
    pub fn has_pypi_dependencies(&self) -> bool {
        self.manifest.has_pypi_dependencies()
    }

    /// Returns the custom location of pypi-name-mapping
    pub fn pypi_name_mapping_source(&self) -> &MappingSource {
        self.mapping_source.get_or_init(|| {
            self.manifest
                .pypi_name_mapping_source(&self.config)
                .expect("mapping source should be ok")
        })
    }

    /// Returns the reqwest client used for http networking
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Create an authenticated reqwest client for this project
    /// use authentication from `rattler_networking`
    pub fn authenticated_client(&self) -> &ClientWithMiddleware {
        &self.authenticated_client
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Return a combination of static environment variables generated from the project and the environment
    /// and from running activation script
    pub async fn get_env_variables(
        &self,
        environment: &Environment<'_>,
    ) -> miette::Result<&HashMap<String, String>> {
        let cell = self.env_vars.get(environment.name()).ok_or_else(|| {
            miette::miette!(
                "{} environment should be already created during project creation",
                environment.name()
            )
        })?;

        cell.get_or_try_init::<miette::Report>(async {
            let activation_env = run_activation(environment).await?;

            let environment_variables = get_environment_variables(environment);

            let all_variables: HashMap<String, String> = activation_env
                .into_iter()
                .chain(environment_variables.into_iter())
                .collect();
            Ok(all_variables)
        })
        .await
    }

    pub(crate) fn task_cache_folder(&self) -> PathBuf {
        self.pixi_dir().join(consts::TASK_CACHE_DIR)
    }
}

/// Iterates over the current directory and all its parent directories and returns the manifest path in the first
/// directory path that contains the [`consts::PROJECT_MANIFEST`] or [`consts::PYPROJECT_MANIFEST`].
pub fn find_project_manifest() -> Option<PathBuf> {
    let current_dir = env::current_dir().ok()?;
    std::iter::successors(Some(current_dir.as_path()), |prev| prev.parent()).find_map(|dir| {
        [PROJECT_MANIFEST, PYPROJECT_MANIFEST]
            .iter()
            .find_map(|manifest| {
                let path = dir.join(manifest);
                if path.is_file() {
                    match *manifest {
                        PROJECT_MANIFEST => Some(path.to_path_buf()),
                        PYPROJECT_MANIFEST if PyProjectToml::is_pixi(&path) => {
                            Some(path.to_path_buf())
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            })
    })
}

/// Create a symlink from the default pixi directory to the custom target directory
#[cfg(not(windows))]
fn create_symlink(pixi_dir_name: &Path, default_pixi_dir: &Path) {
    if default_pixi_dir.exists() {
        tracing::debug!(
            "Symlink already exists at '{}', skipping creating symlink.",
            default_pixi_dir.display()
        );
        return;
    }
    symlink(pixi_dir_name, default_pixi_dir)
        .map_err(|e| {
            tracing::error!(
                "Failed to create symlink from '{}' to '{}': {}",
                pixi_dir_name.display(),
                default_pixi_dir.display(),
                e
            )
        })
        .ok();
}

/// Write a warning file to the default pixi directory to inform the user that symlinks are not supported on this platform (Windows).
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
    use self::has_features::HasFeatures;
    use super::*;
    use crate::project::manifest::FeatureName;
    use insta::{assert_debug_snapshot, assert_snapshot};
    use itertools::Itertools;
    use rattler_conda_types::Platform;
    use rattler_virtual_packages::{LibC, VirtualPackage};
    use std::str::FromStr;

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

    fn format_dependencies(deps: CondaDependencies) -> String {
        deps.iter_specs()
            .map(|(name, spec)| format!("{} = \"{}\"", name.as_source(), spec))
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

        // Using known files in the project so the test succeed including the file check.
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
        // Using known files in the project so the test succeed including the file check.
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
}
