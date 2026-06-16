mod conda_pypi_map;
mod discovery;
mod environment;
pub mod errors;
pub mod grouped_environment;
mod has_project_ref;
pub mod registry;
mod repodata;
mod solve_group;
pub mod virtual_packages;
mod workspace_mut;

use self::errors::VariantsError;
#[cfg(not(windows))]
use std::os::unix::fs::symlink;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::{Debug, Formatter},
    hash::Hash,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    activation::{CurrentEnvVarBehavior, initialize_env_variables},
    lock_file::filter_lock_file,
    repodata::Repodata,
};
use async_once_cell::OnceCell as AsyncCell;
pub use discovery::{DiscoveryStart, WorkspaceLocator, WorkspaceLocatorError};
pub use environment::Environment;
pub use has_project_ref::HasWorkspaceRef;
use indexmap::Equivalent;
use miette::IntoDiagnostic;
use once_cell::sync::OnceCell;
use pep508_rs::Requirement;
use pixi_build_frontend::BackendOverride;
use pixi_command_dispatcher::{CacheDirs, CommandDispatcher, CommandDispatcherBuilder, Limits};
use pixi_config::{Config, RunPostLinkScripts};
use pixi_consts::consts;
use pixi_diff::LockFileDiff;
use pixi_manifest::{
    AssociateProvenance, BuildVariantSource, EnvironmentName, Environments, HasWorkspaceManifest,
    LoadManifestsError, ManifestProvenance, Manifests, PackageManifest, PixiPlatform,
    PixiPlatformName, SpecType, WithProvenance, WithWarnings, WorkspaceManifest,
};
use pixi_path::AbsPathBuf;
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::SourceSpec;
use pixi_utils::reqwest::build_lazy_reqwest_clients;
use pixi_utils::{
    reqwest::LazyReqwestClient,
    variants::{VariantConfig, VariantValue},
};
use pypi_mapping::PurlDerivationMode;
use rattler_conda_types::{
    ChannelConfig, GenericVirtualPackage, MatchSpec, PackageName, Platform, Version,
};
use rattler_lock::LockFile;

use crate::lock_file::LockedPackageKind;
use rattler_networking::{LazyClient, s3_middleware};
use rattler_repodata_gateway::Gateway;
use rattler_virtual_packages::{
    Cuda, EnvOverride, LibC, Linux, Osx, Override, VirtualPackageOverrides, VirtualPackages,
};
pub use registry::{WorkspaceRegistry, WorkspaceRegistryError};
pub use solve_group::SolveGroup;
use tokio::sync::Semaphore;
pub use workspace_mut::WorkspaceMut;
use xxhash_rust::xxh3::xxh3_64;

static CUSTOM_TARGET_DIR_WARN: OnceCell<()> = OnceCell::new();
static CUSTOM_BUILD_DIR_WARN: OnceCell<()> = OnceCell::new();

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

    /// The name of the workspace based on the location of the workspace.
    /// This is used to determine the name of the workspace when no name is
    /// specified.
    manifest_location_name: Option<String>,

    /// Reqwest client shared for this workspace.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    // TODO: once https://github.com/rust-lang/rust/issues/109737 is stabilized, switch to OnceLock
    client: OnceCell<(LazyReqwestClient, rattler_networking::LazyClient)>,

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
    derivation_mode: OnceCell<PurlDerivationMode>,

    /// The global configuration as loaded from the config file(s)
    config: Config,

    /// The S3 configuration
    s3_config: HashMap<String, s3_middleware::S3Config>,

    /// The concurrent request semaphore
    concurrent_downloads_semaphore: OnceCell<Arc<Semaphore>>,

    /// Optional backend override for testing purposes
    backend_override: Option<BackendOverride>,
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
    PypiPackageName,
    (
        Requirement,
        Option<PixiPypiSpec>,
        Option<pixi_manifest::PypiDependencyLocation>,
    ),
>;

pub type MatchSpecs = indexmap::IndexMap<PackageName, (MatchSpec, SpecType)>;
pub type SourceSpecs = indexmap::IndexMap<PackageName, (SourceSpec, SpecType)>;

/// Where the virtual packages of a host [`PixiPlatform`] come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformSource {
    /// Pixi's fixed per-subdir defaults: deterministic and machine-independent.
    Defaults,
    /// The virtual packages actually detected on this machine.
    AutoDetected,
}

/// Whether environment-variable overrides are honored when building a host
/// [`PixiPlatform`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformOverrides {
    /// Ignore `PIXI_OVERRIDE_PLATFORM` and `CONDA_OVERRIDE_*`.
    NoOverrides,
    /// Honor `PIXI_OVERRIDE_PLATFORM` for the subdir and `CONDA_OVERRIDE_*` for
    /// the detected virtual packages (via [`VirtualPackageOverrides::from_env`]).
    EnvironmentVariableOverrides,
}

/// Apply `CONDA_OVERRIDE_*` env vars to `packages`, matching upstream rattler
/// semantics: unset keeps the current version, non-empty replaces it (adding
/// the package if it wasn't detected at all), and empty removes the package
/// entirely. Rattler drives this per slot via `detect_with_fallback`;
/// `Ok(Some(v))` = use v, `Ok(None)` = disabled, error = leave untouched.
fn apply_environment_variable_overrides(packages: &mut Vec<GenericVirtualPackage>) {
    let env = Override::DefaultEnvVar;
    packages.retain_mut(|package| {
        let base = package.version.clone();
        let outcome: Option<Option<Version>> = match package.name.as_normalized() {
            "__cuda" => Cuda::detect_with_fallback(&env, || Ok(Some(Cuda { version: base })))
                .ok()
                .map(|cuda| cuda.map(|cuda| cuda.version)),
            "__linux" => Linux::detect_with_fallback(&env, || Ok(Some(Linux { version: base })))
                .ok()
                .map(|linux| linux.map(|linux| linux.version)),
            "__osx" => Osx::detect_with_fallback(&env, || Ok(Some(Osx { version: base })))
                .ok()
                .map(|osx| osx.map(|osx| osx.version)),
            // The libc family is handled by `apply_glibc_override` below, since
            // the single glibc env var must not rewrite `__musl`/`__eglibc`.
            _ => None,
        };
        match outcome {
            // Override (or unset fallback) produced a version: keep it.
            Some(Some(version)) => {
                package.version = version;
                true
            }
            // Variable was set empty: disable the package.
            Some(None) => false,
            // Not env-overridable, or detection failed: leave untouched.
            None => true,
        }
    });

    // Overrides can introduce packages the machine lacks (`CONDA_OVERRIDE_CUDA`
    // without a GPU), matching rattler; the `Ok(None)` fallback adds only set vars.
    let mut add_missing = |name: &str, version: Option<Version>| {
        let Some(version) = version else { return };
        if packages.iter().any(|p| p.name.as_normalized() == name) {
            return;
        }
        packages.push(GenericVirtualPackage {
            name: name.parse().expect("static virtual package name is valid"),
            version,
            build_string: "0".to_string(),
        });
    };
    add_missing(
        "__cuda",
        Cuda::detect_with_fallback(&env, || Ok(None))
            .ok()
            .flatten()
            .map(|cuda| cuda.version),
    );
    add_missing(
        "__osx",
        Osx::detect_with_fallback(&env, || Ok(None))
            .ok()
            .flatten()
            .map(|osx| osx.version),
    );
    add_missing(
        "__linux",
        Linux::detect_with_fallback(&env, || Ok(None))
            .ok()
            .flatten()
            .map(|linux| linux.version),
    );

    apply_glibc_override(packages);
}

/// Apply `CONDA_OVERRIDE_GLIBC` (rattler's only libc slot) to `packages`. The
/// glibc env var governs glibc alone: unset leaves libc packages untouched, an
/// empty value removes `__glibc`, and a concrete version pins
/// `__glibc=<version>=0` and drops `__musl`/`__eglibc` (one libc family
/// applies).
fn apply_glibc_override(packages: &mut Vec<GenericVirtualPackage>) {
    // Read the variable rattler would and reuse its empty-vs-version parsing.
    let Ok(value) = std::env::var(LibC::DEFAULT_ENV_NAME) else {
        return;
    };
    match LibC::parse_version_opt(&value) {
        // `CONDA_OVERRIDE_GLIBC=""`: drop `__glibc`, leave `__musl`/`__eglibc`.
        Ok(None) => packages.retain(|p| p.name.as_normalized() != "__glibc"),
        // `CONDA_OVERRIDE_GLIBC=<version>`: glibc becomes the active libc.
        Ok(Some(libc)) => {
            packages.retain(|p| !matches!(p.name.as_normalized(), "__musl" | "__eglibc"));
            if let Some(glibc) = packages
                .iter_mut()
                .find(|p| p.name.as_normalized() == "__glibc")
            {
                glibc.version = libc.version;
                glibc.build_string = "0".to_string();
            } else {
                packages.push(GenericVirtualPackage {
                    name: "__glibc"
                        .parse()
                        .expect("static virtual package name is valid"),
                    version: libc.version,
                    build_string: "0".to_string(),
                });
            }
        }
        // Unparsable value: leave the detected packages untouched.
        Err(_) => {}
    }
}

impl Workspace {
    /// Core constructor: takes parsed manifests and loads the workspace config
    /// using `source` for the system + user-level layer.
    pub(crate) fn from_manifests(
        manifest: Manifests,
        source: &pixi_config::GlobalConfigSource,
    ) -> Self {
        let env_vars = Workspace::init_env_vars(&manifest.workspace.value.environments);
        // Get the absolute path of the manifest, preserving symlinks by only
        // canonicalizing the parent directory
        let manifest_path = manifest.workspace.provenance.absolute_path();
        // Take the parent after canonicalizing to ensure this works even when the
        // manifest
        let root = manifest_path
            .parent()
            .expect("manifest path should always have a parent")
            .to_owned();

        // Determine the name of the workspace based on the location of the manifest.
        let manifest_location_name = root.file_name().map(|p| p.to_string_lossy().into_owned());

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

        let config = Config::load_with(&root, source);
        Self {
            root,
            manifest_location_name,
            client: Default::default(),
            workspace: manifest.workspace,
            package: manifest.package,
            env_vars,
            derivation_mode: Default::default(),
            config,
            s3_config,
            repodata_gateway: Default::default(),
            concurrent_downloads_semaphore: OnceCell::default(),
            backend_override: None,
        }
    }

    /// Loads a workspace from a manifest file using the default global-config
    /// search. Pass a source to [`Workspace::from_path_with_source`] to honor
    /// `--no-config` / `--config-file`.
    pub fn from_path(manifest_path: &Path) -> Result<Self, LoadManifestsError> {
        Self::from_path_with_source(manifest_path, &pixi_config::GlobalConfigSource::Search)
    }

    /// Loads a workspace from a manifest file, using `source` for the global
    /// config layer.
    pub fn from_path_with_source(
        manifest_path: &Path,
        source: &pixi_config::GlobalConfigSource,
    ) -> Result<Self, LoadManifestsError> {
        let WithWarnings {
            value: manifests, ..
        } = Manifests::from_workspace_manifest_path(manifest_path.to_path_buf())?;
        Ok(Self::from_manifests(manifests, source))
    }

    /// Constructs a workspace from a manifest string loaded from a specific
    /// location. Uses the default global-config search.
    pub fn from_str(manifest_path: &Path, content: &str) -> Result<Self, LoadManifestsError> {
        let WithWarnings {
            value: manifests, ..
        } = Manifests::from_workspace_source(
            content.with_provenance(ManifestProvenance::from_path(manifest_path.to_path_buf())?),
        )?;
        Ok(Self::from_manifests(
            manifests,
            &pixi_config::GlobalConfigSource::Search,
        ))
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

    pub fn with_cli_config<C>(mut self, config: C) -> Self
    where
        C: Into<Config>,
    {
        self.config = self.config.merge_config(config.into());
        self
    }

    /// Sets the backend override for this workspace. This is primarily used
    /// for testing purposes to inject custom build backends.
    pub fn with_backend_override(mut self, backend_override: BackendOverride) -> Self {
        self.backend_override = Some(backend_override);
        self
    }

    pub fn modify(self) -> Result<WorkspaceMut, LoadManifestsError> {
        WorkspaceMut::new(self)
    }

    /// Returns the display name of the workspace. This name should be used to
    /// provide context to a user.
    ///
    /// This is the name of the workspace as defined in the manifest, or if no
    /// name is specified the name of the root directory of the workspace.
    ///
    /// If the name of the root directory could not be determined, "workspace"
    /// is used as a fallback.
    pub fn display_name(&self) -> &str {
        self.workspace
            .value
            .workspace
            .name
            .as_deref()
            .or(self.manifest_location_name.as_deref())
            .unwrap_or("workspace")
    }

    /// Returns the root directory of the workspace
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the default pixi directory of the workspace [consts::PIXI_DIR],
    /// always pointing to `.pixi` regardless of detached-environments configuration.
    pub fn default_pixi_dir(&self) -> PathBuf {
        self.root.join(consts::PIXI_DIR)
    }

    /// Returns the effective pixi directory for the workspace. When
    /// detached-environments is configured, this returns the project-specific
    /// detached path instead of the default `.pixi` directory.
    pub fn pixi_dir(&self) -> PathBuf {
        self.detached_environments_path()
            .unwrap_or_else(|| self.default_pixi_dir())
    }

    /// Create the detached-environments path for this project if it is set in
    /// the config
    fn detached_environments_path(&self) -> Option<PathBuf> {
        if let Ok(Some(detached_environments_path)) = self.config().detached_environments_dir() {
            Some(detached_environments_path.join(format!(
                "{}-{}",
                self.display_name(),
                xxh3_64(self.root.to_string_lossy().as_bytes())
            )))
        } else {
            None
        }
    }

    /// Returns the default environment directory without interacting with
    /// config.
    pub fn default_environments_dir(&self) -> PathBuf {
        self.default_pixi_dir().join(consts::ENVIRONMENTS_DIR)
    }

    /// Returns the environment directory
    pub fn environments_dir(&self) -> PathBuf {
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
            if !default_envs_dir.is_symlink()
                && self
                    .environments()
                    .iter()
                    .any(|env| default_envs_dir.join(env.name().as_str()).exists())
            {
                let _ = CUSTOM_TARGET_DIR_WARN.get_or_init(|| {
                    tracing::warn!(
                        "Environments found in '{}', this will be ignored and the environment will be installed in the 'detached-environments' directory: '{}'. It's advised to remove the {} folder from the default directory to avoid confusion{}.",
                        default_envs_dir.display(),
                        detached_environments_path.parent().expect("path should have parent").display(),
                        format!("{}/{}", consts::PIXI_DIR, consts::ENVIRONMENTS_DIR),
                        if cfg!(windows) { "" } else { " as a symlink can be made, please re-install after removal." }
                    );
                });
            } else {
                #[cfg(not(windows))]
                create_symlink(&detached_environments_path, &default_envs_dir);
            }

            #[cfg(windows)]
            write_warning_file(
                &default_envs_dir,
                &detached_environments_path,
                "Environments",
                &format!("{}/{}", consts::PIXI_DIR, consts::ENVIRONMENTS_DIR),
            );

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
    pub fn default_solve_group_environments_dir(&self) -> PathBuf {
        self.default_pixi_dir()
            .join(consts::SOLVE_GROUP_ENVIRONMENTS_DIR)
    }

    /// Returns the solve group environments directory
    pub fn solve_group_environments_dir(&self) -> PathBuf {
        self.pixi_dir().join(consts::SOLVE_GROUP_ENVIRONMENTS_DIR)
    }

    /// Returns the default build cache directory without interacting with config.
    pub fn default_build_dir(&self) -> PathBuf {
        self.default_pixi_dir().join(consts::WORKSPACE_CACHE_DIR)
    }

    /// Returns the build cache directory. When detached-environments is
    /// configured, this returns the detached path and creates a symlink from
    /// the default `.pixi/build` location.
    pub fn build_dir(&self) -> PathBuf {
        let default_build_dir = self.default_build_dir();

        // Early out if detached-environments is not set
        if self.config().detached_environments().is_false() {
            return default_build_dir;
        }

        if self.detached_environments_path().is_some() {
            let detached_build_path = self.pixi_dir().join(consts::WORKSPACE_CACHE_DIR);
            if !default_build_dir.is_symlink() && default_build_dir.exists() {
                let _ = CUSTOM_BUILD_DIR_WARN.get_or_init(|| {
                    tracing::warn!(
                        "Build cache found in '{}', this will be ignored and build artifacts will be stored in the 'detached-environments' directory: '{}'. It's advised to remove the {} folder from the default directory to avoid confusion{}.",
                        default_build_dir.display(),
                        detached_build_path.parent().expect("path should have parent").display(),
                        format!("{}/{}", consts::PIXI_DIR, consts::WORKSPACE_CACHE_DIR),
                        if cfg!(windows) { "" } else { " as a symlink can be made, please re-install after removal." }
                    );
                });
            } else {
                #[cfg(not(windows))]
                create_symlink(&detached_build_path, &default_build_dir);
            }

            #[cfg(windows)]
            write_warning_file(
                &default_build_dir,
                &detached_build_path,
                "Build artifacts",
                &format!("{}/{}", consts::PIXI_DIR, consts::WORKSPACE_CACHE_DIR),
            );

            return detached_build_path;
        }

        default_build_dir
    }

    /// Returns the path to the lock file of the project
    /// [consts::PROJECT_LOCK_FILE]
    pub fn lock_file_path(&self) -> PathBuf {
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
    pub fn environments(&self) -> Vec<Environment<'_>> {
        self.workspace
            .value
            .environments
            .iter()
            .map(|env| Environment::new(self, env))
            .collect()
    }

    /// Returns a HashMap of environments in this project.
    pub fn named_environments(&self) -> HashMap<EnvironmentName, Environment<'_>> {
        self.environments()
            .iter()
            .map(|env| (env.name().clone(), env.clone()))
            .collect()
    }

    /// Returns an environment in this project based on a name or an environment
    /// variable.
    ///
    /// If no explicit name is provided, this function will try to read the
    /// environment name from the `PIXI_ENVIRONMENT_NAME` environment variable.
    /// However, if `PIXI_PROJECT_ROOT` is set and differs from this workspace's
    /// root, the environment variable is ignored and the default environment
    /// is returned instead. This handles the case where a pixi task runs
    /// another pixi project via `--manifest-path` - the child process should
    /// not inherit the parent's environment name.
    pub fn environment_from_name_or_env_var(
        &self,
        name: Option<String>,
    ) -> miette::Result<Environment<'_>> {
        let environment_name =
            EnvironmentName::from_arg_or_env_var(name, self.root()).into_diagnostic()?;

        self.environment(&environment_name)
            .ok_or_else(|| miette::miette!("unknown environment '{environment_name}'"))
    }

    /// Returns all the solve groups in the project.
    pub(crate) fn solve_groups(&self) -> Vec<SolveGroup<'_>> {
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
    pub(crate) fn solve_group(&self, name: &str) -> Option<SolveGroup<'_>> {
        self.workspace
            .value
            .solve_groups
            .find(name)
            .map(|group| SolveGroup {
                workspace: self,
                solve_group: group,
            })
    }

    /// Returns the resolved variant configuration for a given platform.
    pub fn variants(&self, platform: &PixiPlatform) -> Result<VariantConfig, VariantsError> {
        // Get inline variants for all targets
        let mut variant_configuration: BTreeMap<String, Vec<VariantValue>> = BTreeMap::new();
        // Resolves from most specific to least specific.
        for build_variants in self
            .workspace
            .value
            .workspace
            .build_variants
            .resolve(Some(platform))
            .flatten()
        {
            // Update the hash map, but only items that are not already in the map.
            for (key, value) in build_variants {
                variant_configuration
                    .entry(key.clone())
                    .or_insert_with(|| value.iter().cloned().map(VariantValue::from).collect());
            }
        }

        // Collect absolute variant file paths without reading their content.
        let variant_files = self
            .workspace
            .value
            .workspace
            .build_variant_files
            .iter()
            .map(|source| match source {
                BuildVariantSource::File(path) => self.root.join(path),
            })
            .collect();

        Ok(VariantConfig {
            variant_configuration,
            variant_files,
        })
    }

    /// Returns the reqwest client used for http networking
    /// this api is not used now, uncomment when use in the future
    pub fn client(&self) -> miette::Result<&LazyReqwestClient> {
        Ok(&self.lazy_client_and_authenticated_client()?.0)
    }

    /// Create an authenticated reqwest client for this project
    /// use authentication from `rattler_networking`
    pub fn authenticated_client(&self) -> miette::Result<&LazyClient> {
        Ok(&self.lazy_client_and_authenticated_client()?.1)
    }

    /// Returns a semaphore than can be used to limit the number of concurrent
    /// according to the user configuration.
    pub fn concurrent_downloads_semaphore(&self) -> Arc<Semaphore> {
        self.concurrent_downloads_semaphore
            .get_or_init(|| {
                let max_concurrent_downloads = self.config().max_concurrent_downloads();
                Arc::new(Semaphore::new(max_concurrent_downloads))
            })
            .clone()
    }

    /// Returns a pre-filled command dispatcher builder. Seeds a
    /// [`RayonPrimer`](crate::rayon_primer::RayonPrimer) in the install /
    /// solve / instantiate-backend reporter slots; UI reporters override.
    pub fn command_dispatcher_builder(&self) -> miette::Result<CommandDispatcherBuilder> {
        let cache_dir = AbsPathBuf::new(pixi_config::get_cache_dir()?)
            .expect("cache dir is not absolute")
            .into_assume_dir();
        let workspace_dir = AbsPathBuf::new(self.pixi_dir())
            .expect("pixi dir is not absolute")
            .into_assume_dir();
        let cache_dirs = CacheDirs::new(cache_dir).with_workspace(workspace_dir);

        // Determine the tool platform to use
        let tool_platform = self.config().tool_platform();
        let host = self.host_platform(
            PlatformSource::Defaults,
            PlatformOverrides::EnvironmentVariableOverrides,
        );
        let tool_virtual_packages =
            if tool_platform.only_platform() == host.subdir().only_platform() {
                // If the tool platform is the same as the current platform, we just assume the
                // same virtual packages apply.
                self.host_platform(
                    PlatformSource::AutoDetected,
                    PlatformOverrides::EnvironmentVariableOverrides,
                )
                .declared_virtual_packages()
                .to_vec()
            } else {
                vec![]
            };

        let root_dir = AbsPathBuf::new(self.root().to_path_buf())
            .expect("root dir is not absolute")
            .into_assume_dir();

        let rayon_primer = std::sync::Arc::new(crate::rayon_primer::RayonPrimer::default());
        Ok(CommandDispatcher::builder()
            .with_gateway(self.repodata_gateway()?.clone())
            .with_cache_dirs(cache_dirs)
            .with_root_dir(root_dir)
            .with_download_client(self.authenticated_client()?.clone())
            .with_max_download_concurrency(self.concurrent_downloads_semaphore())
            .with_limits(Limits {
                max_concurrent_solves: self.config().max_concurrent_solves().into(),
                ..Limits::default()
            })
            .with_backend_overrides(
                self.backend_override
                    .clone()
                    .or_else(|| BackendOverride::from_env().ok().flatten())
                    .unwrap_or_default(),
            )
            .with_channel_config(self.channel_config())
            .execute_link_scripts(match self.config.run_post_link_scripts() {
                RunPostLinkScripts::Insecure => true,
                RunPostLinkScripts::False => false,
            })
            .with_allow_symbolic_links(self.config.allow_symbolic_links)
            .with_allow_hard_links(self.config.allow_hard_links)
            .with_allow_ref_links(self.config.allow_ref_links)
            .with_pixi_install_reporter(rayon_primer.clone())
            .with_pixi_solve_reporter(rayon_primer.clone())
            .with_instantiate_backend_reporter(rayon_primer)
            .with_tool_platform(tool_platform, tool_virtual_packages))
    }

    fn lazy_client_and_authenticated_client(
        &self,
    ) -> miette::Result<&(LazyReqwestClient, rattler_networking::LazyClient)> {
        self.client.get_or_try_init(|| {
            build_lazy_reqwest_clients(Some(self.config()), Some(self.s3_config.clone()))
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The platform pixi treats as this machine's host.
    ///
    /// `source` selects whether the virtual packages are pixi's per-subdir
    /// defaults (deterministic) or the set actually detected on this machine;
    /// `overrides` selects whether the `PIXI_OVERRIDE_PLATFORM` (subdir) and
    /// `CONDA_OVERRIDE_*` (virtual package) environment variables are honored.
    pub fn host_platform(
        &self,
        source: PlatformSource,
        overrides: PlatformOverrides,
    ) -> PixiPlatform {
        let subdir = match overrides {
            PlatformOverrides::NoOverrides => Platform::current(),
            PlatformOverrides::EnvironmentVariableOverrides => {
                std::env::var(consts::PIXI_OVERRIDE_PLATFORM)
                    .ok()
                    .and_then(|value| match value.parse::<Platform>() {
                        Ok(platform) => Some(platform),
                        Err(_) => {
                            tracing::warn!(
                                "Invalid value for PIXI_OVERRIDE_PLATFORM='{value}', ignoring."
                            );
                            None
                        }
                    })
                    .unwrap_or_else(Platform::current)
            }
        };

        let mut virtual_packages = match source {
            PlatformSource::Defaults => PixiPlatform::from_subdir(subdir)
                .declared_virtual_packages()
                .to_vec(),
            PlatformSource::AutoDetected => {
                VirtualPackages::detect(&VirtualPackageOverrides::default())
                    .map(|detected| detected.into_generic_virtual_packages().collect())
                    .unwrap_or_default()
            }
        };

        if let PlatformOverrides::EnvironmentVariableOverrides = overrides {
            apply_environment_variable_overrides(&mut virtual_packages);
        }

        PixiPlatform::from_required_virtual_packages(subdir, virtual_packages)
    }

    /// Construct a [`ChannelConfig`] that is specific to this project. This
    /// ensures that the root directory is set correctly.
    pub fn channel_config(&self) -> ChannelConfig {
        ChannelConfig {
            root_dir: self.root.clone(),
            ..self.config.global_channel_config().clone()
        }
    }

    pub fn task_cache_folder(&self) -> PathBuf {
        self.pixi_dir().join(consts::TASK_CACHE_DIR)
    }

    pub fn activation_env_cache_folder(&self) -> PathBuf {
        self.pixi_dir().join(consts::ACTIVATION_ENV_CACHE_DIR)
    }

    /// Returns which PyPI purl derivation mode we should use.
    /// It can use project-defined mappings in the format `conda_name: pypi_name`,
    /// or the self-hosted prefix.dev mappings.
    pub fn pypi_name_derivation_mode(&self) -> miette::Result<&PurlDerivationMode> {
        self.derivation_mode.get_or_try_init(|| {
            conda_pypi_map::build_pypi_name_derivation_mode(
                &self.workspace.value,
                &self.channel_config(),
            )
        })
    }

    /// Constructs a new lock file where some of the constraints have been
    /// removed.
    fn unlock_packages(
        &self,
        lock_file: &LockFile,
        conda_packages: HashSet<PackageName>,
        pypi_packages: HashSet<pep508_rs::PackageName>,
        affected_environments: HashSet<(&str, PixiPlatformName)>,
    ) -> LockFile {
        filter_lock_file(self, lock_file, |env, platform, package| {
            if affected_environments.contains(&(env.name().as_str(), platform.clone())) {
                match package {
                    LockedPackageKind::Conda(name) => !conda_packages.contains(name),
                    LockedPackageKind::Pypi(name) => !pypi_packages.contains(name),
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

/// Create or update a symlink from the directory to the custom target directory.
#[cfg(not(windows))]
fn create_symlink(target_dir: &Path, symlink_dir: &Path) {
    match fs_err::symlink_metadata(symlink_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => match fs_err::read_link(symlink_dir) {
            Ok(existing_target) if existing_target == target_dir => {
                tracing::debug!(
                    "Symlink already exists at '{}', skipping creating symlink.",
                    symlink_dir.display()
                );
                return;
            }
            Ok(existing_target) => {
                tracing::debug!(
                    "Symlink at '{}' points to '{}', updating it to '{}'.",
                    symlink_dir.display(),
                    existing_target.display(),
                    target_dir.display()
                );
                if let Err(e) = fs_err::remove_file(symlink_dir) {
                    tracing::error!(
                        "Failed to remove symlink '{}': {}",
                        symlink_dir.display(),
                        e
                    );
                    return;
                }
            }
            Err(e) => {
                tracing::error!("Failed to read symlink '{}': {}", symlink_dir.display(), e);
                return;
            }
        },
        Ok(_) => {
            tracing::debug!(
                "Path already exists at '{}', skipping creating symlink.",
                symlink_dir.display()
            );
            return;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::error!(
                "Failed to inspect symlink '{}': {}",
                symlink_dir.display(),
                e
            );
            return;
        }
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

/// Write or update a warning file to inform the user that symlinks are not
/// supported on this platform (Windows).
#[cfg(windows)]
fn write_warning_file(
    default_dir: &Path,
    target_dir: &Path,
    contents_name: &str,
    default_dir_name: &str,
) {
    let warning_file = default_dir.join("README.txt");
    let warning_message = format!(
        "{} are stored in a custom detached-environments directory: {}.\n\
        Symlinks are not supported on this platform so they will not be reachable from the default ('{}') directory.",
        contents_name,
        target_dir.display(),
        default_dir_name
    );
    match fs_err::read_to_string(&warning_file) {
        Ok(existing_message) if existing_message == warning_message => {
            tracing::debug!(
                "Symlink warning file already exists at '{}', skipping writing warning file.",
                warning_file.display()
            );
            return;
        }
        Ok(_) => {
            tracing::debug!(
                "Symlink warning file at '{}' is stale, updating it.",
                warning_file.display()
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::debug!(
                "Failed to read symlink warning file at '{}': {}",
                warning_file.display(),
                e
            );
        }
    }

    // Create directory if it doesn't exist
    if let Err(e) = fs_err::create_dir_all(default_dir) {
        tracing::error!(
            "Failed to create directory '{}': {}",
            default_dir.display(),
            e
        );
        return;
    }

    // Write warning message to file
    match fs_err::write(&warning_file, &warning_message) {
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
    use pixi_config::{Config, DetachedEnvironments};
    use pixi_manifest::{FeatureName, FeaturesExt, HasWorkspaceManifest};
    use pypi_mapping::{ProjectDefinedChannelMapping, ProjectDefinedMappingLocation};
    use rattler_conda_types::{Channel, Platform, Version};
    use url::Url;
    use xxhash_rust::xxh3::xxh3_64;

    use super::*;

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = ["linux-64", "win-64"]
        "#;

    /// `CONDA_OVERRIDE_*` must be able to *introduce* a virtual package the
    /// machine doesn't provide (e.g. cuda on a GPU-less box), not just
    /// override detected ones.
    #[test]
    fn override_adds_undetected_virtual_package() {
        let packages = temp_env::with_var("CONDA_OVERRIDE_CUDA", Some("12.0"), || {
            let mut packages = Vec::new();
            apply_environment_variable_overrides(&mut packages);
            packages
        });

        let cuda = packages
            .iter()
            .find(|p| p.name.as_normalized() == "__cuda")
            .expect("__cuda should be added from the override");
        assert_eq!(cuda.version, Version::from_str("12.0").unwrap());
    }

    fn libc_package(name: &str, version: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: name.parse().unwrap(),
            version: Version::from_str(version).unwrap(),
            build_string: "0".to_string(),
        }
    }

    fn has_package(packages: &[GenericVirtualPackage], name: &str) -> bool {
        packages.iter().any(|p| p.name.as_normalized() == name)
    }

    /// An empty `CONDA_OVERRIDE_GLIBC` drops `__glibc` but must leave a
    /// non-glibc libc family (here `__musl`) untouched -- the glibc slot only
    /// governs glibc.
    #[test]
    fn empty_glibc_override_drops_glibc_but_keeps_musl() {
        let packages = temp_env::with_var("CONDA_OVERRIDE_GLIBC", Some(""), || {
            let mut packages = vec![
                libc_package("__glibc", "2.28"),
                libc_package("__musl", "1.2"),
            ];
            apply_environment_variable_overrides(&mut packages);
            packages
        });

        assert!(!has_package(&packages, "__glibc"));
        assert!(has_package(&packages, "__musl"));
    }

    /// A `CONDA_OVERRIDE_GLIBC` version makes glibc the active libc: it pins
    /// `__glibc=<version>=0` and displaces any detected `__musl`/`__eglibc`.
    #[test]
    fn glibc_version_override_displaces_other_libc_families() {
        let packages = temp_env::with_var("CONDA_OVERRIDE_GLIBC", Some("2.40"), || {
            let mut packages = vec![
                libc_package("__musl", "1.2"),
                libc_package("__eglibc", "2.30"),
            ];
            apply_environment_variable_overrides(&mut packages);
            packages
        });

        assert!(!has_package(&packages, "__musl"));
        assert!(!has_package(&packages, "__eglibc"));
        let glibc = packages
            .iter()
            .find(|p| p.name.as_normalized() == "__glibc")
            .expect("a glibc version override should add __glibc");
        assert_eq!(glibc.version, Version::from_str("2.40").unwrap());
        assert_eq!(glibc.build_string, "0");
    }

    /// Every legacy `[system-requirements]` shape parses through the
    /// `[system-requirements]`-to-platforms migration and ends up as a
    /// synthesised platform declaring `__glibc=2.12`. Exercises the
    /// toml-span parser's accepted shapes by way of the observable migration
    /// output rather than via the now-private SystemRequirements field.
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
            let glibc_platform = (&workspace)
                .workspace_manifest()
                .workspace
                .platforms
                .iter()
                .find(|p| {
                    p.declared_virtual_packages()
                        .iter()
                        .any(|g| g.name.as_normalized() == "__glibc")
                })
                .expect("the migration should synthesise a platform carrying __glibc");
            let glibc = glibc_platform
                .declared_virtual_packages()
                .iter()
                .find(|g| g.name.as_normalized() == "__glibc")
                .unwrap();
            assert_eq!(glibc.version, Version::from_str("2.12").unwrap());
        }
    }

    #[test]
    fn test_workspace_name_when_specified() {
        const WORKSPACE_STR: &str = r#"
        [workspace]
        name = "foo"
        channels = []
        "#;

        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::from_str(
            &temp_dir.path().join(consts::WORKSPACE_MANIFEST),
            WORKSPACE_STR,
        )
        .unwrap();
        assert_eq!(workspace.display_name(), "foo");
    }

    #[test]
    fn test_workspace_name_when_unspecified() {
        const WORKSPACE_STR: &str = r#"
        [workspace]
        channels = []
        "#;

        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::from_str(
            &temp_dir
                .path()
                .join("foobar")
                .join(consts::WORKSPACE_MANIFEST),
            WORKSPACE_STR,
        )
        .unwrap();
        assert_eq!(workspace.display_name(), "foobar");
    }

    #[test]
    fn test_workspace_name_when_undefined() {
        const WORKSPACE_STR: &str = r#"
        [workspace]
        channels = []
        "#;

        let workspace = Workspace::from_str(
            &Path::new("/").join(consts::WORKSPACE_MANIFEST),
            WORKSPACE_STR,
        )
        .unwrap();
        assert_eq!(workspace.display_name(), "workspace");
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

        let linux64 = pixi_manifest::PixiPlatform::from_subdir(Platform::Linux64);
        assert_snapshot!(format_dependencies(
            workspace
                .default_environment()
                .combined_dependencies(Some(&linux64))
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

        let linux64 = pixi_manifest::PixiPlatform::from_subdir(Platform::Linux64);
        assert_snapshot!(format_dependencies(
            workspace
                .default_environment()
                .combined_dependencies(Some(&linux64))
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

        let linux64 = pixi_manifest::PixiPlatform::from_subdir(Platform::Linux64);
        assert_snapshot!(format_dependencies(
            workspace
                .default_environment()
                .combined_dependencies(Some(&linux64))
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

        let linux64 = pixi_manifest::PixiPlatform::from_subdir(Platform::Linux64);
        let win64 = pixi_manifest::PixiPlatform::from_subdir(Platform::Win64);
        let osx_arm64 = pixi_manifest::PixiPlatform::from_subdir(Platform::OsxArm64);
        assert_snapshot!(format!(
            "= Linux64\n{}\n\n= Win64\n{}\n\n= OsxArm64\n{}",
            fmt_activation_scripts(
                workspace
                    .default_environment()
                    .activation_scripts(Some(&linux64))
            ),
            fmt_activation_scripts(
                workspace
                    .default_environment()
                    .activation_scripts(Some(&win64))
            ),
            fmt_activation_scripts(
                workspace
                    .default_environment()
                    .activation_scripts(Some(&osx_arm64))
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

        let osx64 = pixi_manifest::PixiPlatform::from_subdir(Platform::Osx64);
        let win64 = pixi_manifest::PixiPlatform::from_subdir(Platform::Win64);
        let linux64 = pixi_manifest::PixiPlatform::from_subdir(Platform::Linux64);
        assert_debug_snapshot!(
            workspace
                .workspace
                .value
                .tasks(Some(&osx64), &FeatureName::DEFAULT)
                .unwrap()
        );
        assert_debug_snapshot!(
            workspace
                .workspace
                .value
                .tasks(Some(&win64), &FeatureName::DEFAULT)
                .unwrap()
        );
        assert_debug_snapshot!(
            workspace
                .workspace
                .value
                .tasks(Some(&linux64), &FeatureName::DEFAULT)
                .unwrap()
        );
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

        let mapping = workspace.pypi_name_derivation_mode().unwrap();
        let channel = Channel::from_str("conda-forge", &workspace.channel_config()).unwrap();
        let canonical_name = channel.canonical_name();

        let canonical_channel_name = canonical_name.trim_end_matches('/');

        assert_eq!(
            mapping
                .project_defined()
                .unwrap()
                .mapping
                .get(canonical_channel_name)
                .unwrap(),
            // Bare location strings use the additive (overlay) mode.
            &ProjectDefinedChannelMapping::extend(ProjectDefinedMappingLocation::Url {
                url: Url::parse(
                    "https://github.com/prefix-dev/parselmouth/blob/main/files/compressed_mapping.json"
                )
                .unwrap(),
                cache_ttl: None,
            })
        );

        // Check url channel as map key
        let file_contents = r#"
            [workspace]
            name = "foo"
            channels = ["https://prefix.dev/test-channel"]
            platforms = []
            conda-pypi-map = {"https://prefix.dev/test-channel" = "mapping.json"}
            "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        let mapping = workspace.pypi_name_derivation_mode().unwrap();
        assert_eq!(
            mapping
                .project_defined()
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
            &ProjectDefinedChannelMapping::extend(ProjectDefinedMappingLocation::Path(
                workspace
                    .channel_config()
                    .root_dir
                    .join(PathBuf::from("mapping.json"))
            ))
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

        assert!(workspace.pypi_name_derivation_mode().is_ok());

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

        insta::assert_snapshot!(workspace.pypi_name_derivation_mode().unwrap_err());
    }

    #[test]
    #[cfg(unix)]
    fn test_workspace_root_preserves_symlink_location() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dotfiles_dir = temp_dir.path().join("dotfiles");
        let home_dir = temp_dir.path().join("home");
        fs_err::create_dir_all(&dotfiles_dir).unwrap();
        fs_err::create_dir_all(&home_dir).unwrap();

        // Real manifest lives inside the dotfiles directory
        let real_manifest = dotfiles_dir.join("pixi.toml");
        fs_err::write(
            &real_manifest,
            r#"
            [workspace]
            name = "test"
            channels = []
            platforms = []
            "#,
        )
        .unwrap();

        // Home directory contains a symlink that points at the real manifest
        let symlink_manifest = home_dir.join("pixi.toml");
        std::os::unix::fs::symlink(&real_manifest, &symlink_manifest).unwrap();

        // Load workspace from the symlinked manifest path
        let workspace = Workspace::from_path(&symlink_manifest).unwrap();

        // The workspace root should be the home_dir (where the symlink lives),
        // NOT the dotfiles_dir (where the real file lives)
        let canonical_home = dunce::canonicalize(&home_dir).unwrap();
        assert_eq!(
            workspace.root(),
            canonical_home,
            "workspace root should be relative to symlink location, not the real file location"
        );

        // The .pixi directory should be created in the home directory
        let expected_pixi_dir = canonical_home.join(consts::PIXI_DIR);
        assert_eq!(
            workspace.pixi_dir(),
            expected_pixi_dir,
            ".pixi directory should be in the symlink's parent directory"
        );
    }

    const WORKSPACE_MANIFEST_STR: &str = r#"[workspace]
name = "myproj"
channels = []
platforms = []
"#;

    #[test]
    fn test_dirs_without_detached() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::from_str(
            &temp_dir.path().join(consts::WORKSPACE_MANIFEST),
            WORKSPACE_MANIFEST_STR,
        )
        .unwrap();

        let dot_pixi = dunce::canonicalize(temp_dir.path()).unwrap().join(".pixi");
        assert_eq!(workspace.default_pixi_dir(), dot_pixi);
        assert_eq!(workspace.pixi_dir(), dot_pixi);
        assert_eq!(
            workspace.default_environments_dir(),
            dot_pixi.join(consts::ENVIRONMENTS_DIR)
        );
        assert_eq!(
            workspace.default_solve_group_environments_dir(),
            dot_pixi.join(consts::SOLVE_GROUP_ENVIRONMENTS_DIR)
        );
        assert_eq!(
            workspace.default_build_dir(),
            dot_pixi.join(consts::WORKSPACE_CACHE_DIR)
        );
        assert_eq!(workspace.build_dir(), workspace.default_build_dir());
    }

    #[test]
    fn test_dirs_with_detached() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let detached_dir = tempfile::tempdir().unwrap();

        let workspace = Workspace::from_str(
            &workspace_dir.path().join(consts::WORKSPACE_MANIFEST),
            WORKSPACE_MANIFEST_STR,
        )
        .unwrap()
        .with_cli_config(Config {
            detached_environments: Some(DetachedEnvironments::Path(
                detached_dir.path().to_path_buf(),
            )),
            ..Default::default()
        });

        let dot_pixi = dunce::canonicalize(workspace_dir.path())
            .unwrap()
            .join(".pixi");
        let detached_subdir = detached_dir.path().join(format!(
            "{}-{}",
            workspace.display_name(),
            xxh3_64(workspace.root().to_string_lossy().as_bytes())
        ));

        // default_* methods always point at local .pixi
        assert_eq!(workspace.default_pixi_dir(), dot_pixi);
        assert_eq!(
            workspace.default_environments_dir(),
            dot_pixi.join(consts::ENVIRONMENTS_DIR)
        );
        assert_eq!(
            workspace.default_solve_group_environments_dir(),
            dot_pixi.join(consts::SOLVE_GROUP_ENVIRONMENTS_DIR)
        );
        assert_eq!(
            workspace.default_build_dir(),
            dot_pixi.join(consts::WORKSPACE_CACHE_DIR)
        );

        // effective paths point into the detached directory
        assert_eq!(workspace.pixi_dir(), detached_subdir);
        assert_eq!(
            workspace.build_dir(),
            detached_subdir.join(consts::WORKSPACE_CACHE_DIR)
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn test_detached_symlinks_follow_config_changes() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let detached_dir_a = tempfile::tempdir().unwrap();
        let detached_dir_b = tempfile::tempdir().unwrap();

        let workspace_with_detached_dir = |detached_dir: &Path| {
            Workspace::from_str(
                &workspace_dir.path().join(consts::WORKSPACE_MANIFEST),
                WORKSPACE_MANIFEST_STR,
            )
            .unwrap()
            .with_cli_config(Config {
                detached_environments: Some(DetachedEnvironments::Path(detached_dir.to_path_buf())),
                ..Default::default()
            })
        };

        let workspace_a = workspace_with_detached_dir(detached_dir_a.path());
        let default_envs_dir = workspace_a.default_environments_dir();
        let default_build_dir = workspace_a.default_build_dir();

        let envs_dir_a = workspace_a.environments_dir();
        let build_dir_a = workspace_a.build_dir();
        assert_eq!(fs_err::read_link(&default_envs_dir).unwrap(), envs_dir_a);
        assert_eq!(fs_err::read_link(&default_build_dir).unwrap(), build_dir_a);

        let workspace_b = workspace_with_detached_dir(detached_dir_b.path());
        let envs_dir_b = workspace_b.environments_dir();
        let build_dir_b = workspace_b.build_dir();

        assert_eq!(fs_err::read_link(default_envs_dir).unwrap(), envs_dir_b);
        assert_eq!(fs_err::read_link(default_build_dir).unwrap(), build_dir_b);
    }

    #[test]
    #[cfg(not(windows))]
    fn test_detached_symlinks_do_not_replace_existing_directories() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let detached_dir = tempfile::tempdir().unwrap();

        let workspace = Workspace::from_str(
            &workspace_dir.path().join(consts::WORKSPACE_MANIFEST),
            WORKSPACE_MANIFEST_STR,
        )
        .unwrap()
        .with_cli_config(Config {
            detached_environments: Some(DetachedEnvironments::Path(
                detached_dir.path().to_path_buf(),
            )),
            ..Default::default()
        });

        let default_envs_dir = workspace.default_environments_dir();
        let default_build_dir = workspace.default_build_dir();
        fs_err::create_dir_all(default_envs_dir.join(consts::DEFAULT_ENVIRONMENT_NAME)).unwrap();
        fs_err::create_dir_all(&default_build_dir).unwrap();

        let envs_dir = workspace.environments_dir();
        let build_dir = workspace.build_dir();

        assert!(envs_dir.starts_with(detached_dir.path()));
        assert!(build_dir.starts_with(detached_dir.path()));
        assert!(!default_envs_dir.is_symlink());
        assert!(!default_build_dir.is_symlink());
        assert!(
            default_envs_dir
                .join(consts::DEFAULT_ENVIRONMENT_NAME)
                .is_dir()
        );
        assert!(default_build_dir.is_dir());
    }

    #[test]
    #[cfg(windows)]
    fn test_detached_warning_file_follows_config_changes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let default_dir = temp_dir.path().join(".pixi").join("envs");
        let warning_file = default_dir.join("README.txt");
        let target_dir_a = temp_dir.path().join("detached-a").join("envs");
        let target_dir_b = temp_dir.path().join("detached-b").join("envs");

        write_warning_file(&default_dir, &target_dir_a, "Environments", ".pixi/envs");
        let warning_a = fs_err::read_to_string(&warning_file).unwrap();
        assert!(warning_a.contains(&target_dir_a.display().to_string()));

        write_warning_file(&default_dir, &target_dir_b, "Environments", ".pixi/envs");
        let warning_b = fs_err::read_to_string(&warning_file).unwrap();
        assert!(warning_b.contains(&target_dir_b.display().to_string()));
        assert!(!warning_b.contains(&target_dir_a.display().to_string()));
    }
}
