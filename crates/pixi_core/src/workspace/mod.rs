mod conda_pypi_map;
mod discovery;
mod environment;
pub mod errors;
pub mod grouped_environment;
mod has_project_ref;
pub mod registry;
mod repodata;
mod solve_group;
pub mod stdlib_variants;
pub mod virtual_packages;
mod workspace_mut;
mod workspace_script;

use self::errors::VariantsError;
use self::workspace_script::{ScriptSource, WorkspaceScript};
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
use indexmap::{Equivalent, IndexSet};
use miette::{Diagnostic, IntoDiagnostic};
use once_cell::sync::OnceCell;
use pep508_rs::Requirement;
use pixi_build_frontend::BackendOverride;
use pixi_command_dispatcher::{CacheDirs, CommandDispatcher, CommandDispatcherBuilder, Limits};
use pixi_conda_script::CondaScriptManifest;
use pixi_config::{CacheKind, Config, RunPostLinkScripts};
use pixi_consts::consts;
use pixi_diff::LockFileDiff;
use pixi_manifest::{
    AssociateProvenance, BuildVariantSource, EnvironmentName, Environments, FeaturesExt,
    HasWorkspaceManifest, LoadManifestsError, ManifestKind, ManifestProvenance, Manifests,
    PackageManifest, PixiPlatform, PixiPlatformName, PrioritizedChannel, SpecType, WithProvenance,
    WithWarnings, WorkspaceManifest,
    script::ScriptManifest,
    toml::{ExternalWorkspaceProperties, FromTomlStr, PackageDefaults, TomlManifest},
    utils::WithSourceCode,
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
    ChannelConfig, ChannelUrl, GenericVirtualPackage, MatchSpec, PackageName, Platform,
};
use rattler_lock::LockFile;
use thiserror::Error;

use crate::lock_file::LockedPackageKind;
use pixi_manifest::platform::host::{
    detect_host, host_capabilities, host_subdir, platform_from_detected,
};
use pixi_manifest::platform::unsatisfied_capabilities;
use rattler_networking::{LazyClient, s3_middleware};
use rattler_repodata_gateway::Gateway;
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

    storage: WorkspaceStorage,

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

#[derive(Debug, Clone)]
enum WorkspaceStorage {
    Project,
    Script(WorkspaceScript),
}

#[derive(Debug, Error, Diagnostic)]
pub enum ScriptWorkspaceError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Manifest(#[from] pixi_manifest::script::ScriptManifestError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    CondaScriptManifest(Box<WithSourceCode<pixi_manifest::TomlError, miette::NamedSource<String>>>),

    #[error("failed to resolve the script environment cache directory: {0}")]
    CacheDirectory(String),

    #[error("failed to determine the virtual packages of this machine for '{subdir}'")]
    #[diagnostic(help(
        "a script without `platforms` is resolved for this machine. Declare the platforms in the script metadata to resolve it for a fixed target instead."
    ))]
    HostDetection {
        subdir: Platform,
        #[source]
        source: pixi_manifest::platform::host::HostDetectionError,
    },
}

/// Install the platforms picked for a script that declares none.
///
/// `use_platform_composition` is decided while parsing, and a script's
/// synthetic manifest parses with an empty `platforms`, which reads as "every
/// platform is a bare subdir". Composition would then resolve an environment's
/// platform by *subdir name* and never find the rich platform injected here, so
/// the flag is recomputed for the platforms actually installed.
fn set_implicit_script_platforms(
    workspace: &mut pixi_manifest::Workspace,
    platforms: IndexSet<PixiPlatform>,
) {
    workspace.use_platform_composition = platforms.iter().all(PixiPlatform::is_subdir_platform);
    workspace.platforms = platforms;
}

/// The platforms a script that declares none is resolved for.
///
/// Such a script is resolved for the machine it runs on, like its no-manifest
/// siblings `pixi exec` and `pixi global`, while a workspace takes "the current
/// platform" to mean the bare subdir with pixi's assumed defaults. Otherwise a
/// script on a machine with CUDA, or with a glibc newer than pixi's 2.28 floor,
/// is solved against packages that machine does not need to be limited to.
///
/// An adjacent lock file wins while it is usable, so a `pixi lock --script`
/// keeps reproducing rather than being re-solved for a marginally different
/// host. Its platforms are rebuilt in full, virtual packages included, since
/// bare subdirs would lose the machine they were locked for. Their names are
/// synthesized from their contents rather than taken from the lock, so a lock
/// written by an older pixi under `p1`/`p2` aliases still maps on;
/// `align_platform_names` matches the rows by identity either way.
///
/// A recorded platform is kept only when it says something this machine can
/// honour. One this machine cannot run, and one that records nothing beyond
/// pixi's defaults for the subdir, both give way to the host and a re-solve.
/// Rows for other subdirs are dropped. All three warn, since the next write to
/// the lock file makes them permanent.
///
/// A lock file that does not parse is left to the loader to report.
fn implicit_script_platforms(
    lock_file_path: Option<&Path>,
) -> Result<IndexSet<PixiPlatform>, ScriptWorkspaceError> {
    let subdir = host_subdir();
    let host = detect_host(subdir).map_err(|error| ScriptWorkspaceError::HostDetection {
        subdir,
        source: error,
    })?;

    // A lock file that does not parse is passed over silently: the loader reads
    // the same file moments later and reports why it is unusable, with a
    // position in the file that is not available here.
    let Some(lock_file) = lock_file_path
        .filter(|path| path.is_file())
        .and_then(|path| LockFile::from_path(path).ok())
    else {
        return Ok(IndexSet::from([host]));
    };

    // A row carrying nothing beyond the subdir baseline records no machine at
    // all, and adopting it would pin the script to pixi's defaults. When the
    // host is itself baseline the two are the same platform, so there is
    // nothing to reject and no re-solve to trigger on every run.
    let host_is_baseline = host.customised_virtual_packages().is_empty();

    let mut foreign_subdirs: IndexSet<Platform> = IndexSet::new();
    let mut rejected_baseline = false;
    let mut rejected_unrunnable = false;
    let mut locked: IndexSet<PixiPlatform> = IndexSet::new();
    for row in lock_file.platforms() {
        // A lock can hold rows for other subdirs, from a script that declared
        // `platforms` and had the line removed since. Keeping those would make
        // every later run solve and lock for a platform it no longer asks for.
        if row.subdir() != subdir {
            foreign_subdirs.insert(row.subdir());
            continue;
        }
        let Ok(recorded) = platform_from_detected(row.subdir(), locked_virtual_packages(&row))
        else {
            continue;
        };
        if !host_is_baseline && recorded.customised_virtual_packages().is_empty() {
            rejected_baseline = true;
            continue;
        }
        if !unsatisfied_capabilities(
            &recorded.customised_virtual_packages(),
            host.declared_virtual_packages(),
        )
        .is_empty()
        {
            rejected_unrunnable = true;
            continue;
        }
        locked.insert(recorded);
    }

    // `--frozen` and `--locked` consume the lock file without writing, so none
    // of these warnings may claim that it *is* rewritten.
    if !foreign_subdirs.is_empty() {
        tracing::warn!(
            "the lock file next to this script also records {}, which a script without \
             `platforms` does not ask for, so the next write to the lock file drops those \
             rows.\n\
             Declare `platforms` in the script metadata to keep locking for them.",
            foreign_subdirs
                .iter()
                .map(|subdir| format!("'{}'", subdir.as_str()))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }

    if locked.is_empty() {
        if rejected_unrunnable {
            tracing::warn!(
                "the lock file next to this script records a platform this machine cannot run, \
                 so the script is resolved for '{}' instead, and the next write to the lock file \
                 replaces what it records.\n\
                 Declare `platforms` in the script metadata to keep locking for a fixed target.",
                host.name().as_str(),
            );
        } else if rejected_baseline {
            tracing::warn!(
                "the lock file next to this script records pixi's defaults for '{}' rather than \
                 the machine it was locked on, so the script is resolved for '{}' instead, and \
                 the next write to the lock file replaces what it records.\n\
                 Declare `platforms` in the script metadata to keep locking for a fixed target.",
                subdir.as_str(),
                host.name().as_str(),
            );
        }
        return Ok(IndexSet::from([host]));
    }

    Ok(locked)
}

/// The virtual packages a lock-file platform row records, as the typed form.
/// Entries the current pixi cannot parse are dropped; they can only have come
/// from a newer pixi, and a platform is defined by what we can compare.
fn locked_virtual_packages(platform: &rattler_lock::Platform<'_>) -> Vec<GenericVirtualPackage> {
    platform
        .virtual_packages()
        .iter()
        .filter_map(|raw| pixi_manifest::platform::parse_locked_virtual_package(raw))
        .collect()
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

impl Workspace {
    /// Core constructor: takes parsed manifests and loads the workspace config
    /// using `source` for the system + user-level layer.
    pub(crate) fn from_manifests(
        manifest: Manifests,
        source: &pixi_config::GlobalConfigSource,
    ) -> Self {
        // Get the absolute path of the manifest, preserving symlinks by only
        // canonicalizing the parent directory
        let manifest_path = manifest.workspace.provenance.absolute_path();
        // Take the parent after canonicalizing to ensure this works even when the
        // manifest
        let root = manifest_path
            .parent()
            .expect("manifest path should always have a parent")
            .to_owned();

        let config = Config::load_with(&root, source);
        Self::from_parsed(
            manifest.workspace,
            manifest.package,
            root,
            config,
            WorkspaceStorage::Project,
        )
    }

    fn from_parsed(
        workspace: WithProvenance<WorkspaceManifest>,
        package: Option<WithProvenance<PackageManifest>>,
        root: PathBuf,
        config: Config,
        storage: WorkspaceStorage,
    ) -> Self {
        let env_vars = Workspace::init_env_vars(&workspace.value.environments);
        let manifest_location_name = root.file_name().map(|p| p.to_string_lossy().into_owned());
        let s3_options = workspace.value.workspace.s3_options.clone();
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

        Self {
            root,
            storage,
            manifest_location_name,
            client: Default::default(),
            workspace,
            package,
            env_vars,
            derivation_mode: Default::default(),
            config,
            s3_config,
            repodata_gateway: Default::default(),
            concurrent_downloads_semaphore: OnceCell::default(),
            backend_override: None,
        }
    }

    /// Construct an isolated workspace for a local PEP 723 script.
    ///
    /// `config` must include both the selected global configuration and CLI overrides so the
    /// cached environment path and default channels are final when the workspace is constructed.
    pub fn from_script(
        script: ScriptManifest,
        config: Config,
    ) -> Result<WithWarnings<Self>, ScriptWorkspaceError> {
        let script_path = script.path().to_owned();
        let script_manifest = script.clone();
        let script_config = script.workspace_config()?;
        let (mut manifest, warnings) = script.into_workspace_manifest()?;

        if !script_config.channels_explicit {
            manifest.workspace.channels = config
                .default_channels()
                .into_iter()
                .map(PrioritizedChannel::from)
                .collect();
        }
        let root = script_path
            .parent()
            .expect("an absolute script path always has a parent")
            .to_owned();
        let cache_root = config
            .cache_dir_for(CacheKind::ExecEnvironments)
            .map_err(|error| ScriptWorkspaceError::CacheDirectory(error.to_string()))?;
        let workspace_script = WorkspaceScript::for_local(script_manifest, &cache_root);
        if !script_config.platforms_explicit {
            let lock_file_path = workspace_script
                .lock_file_path()
                .expect("a local script has an adjacent lock-file path");
            set_implicit_script_platforms(
                &mut manifest.workspace,
                implicit_script_platforms(Some(&lock_file_path))?,
            );
        }
        let workspace =
            manifest.with_provenance(ManifestProvenance::new(script_path, ManifestKind::Pep723));

        Ok(WithWarnings::from(Self::from_parsed(
            workspace,
            None,
            root,
            config,
            WorkspaceStorage::Script(workspace_script),
        ))
        .with_warnings(warnings))
    }

    /// Construct an isolated workspace for a local `conda-script` file.
    ///
    /// `config` must include both the selected global configuration and CLI
    /// overrides, like [`Workspace::from_script`].
    pub fn from_conda_script(
        script: CondaScriptManifest,
        config: Config,
    ) -> Result<WithWarnings<Self>, ScriptWorkspaceError> {
        let script_path = script.path().to_owned();
        let root = script_path
            .parent()
            .expect("an absolute script path always has a parent")
            .to_owned();
        // The diagnostics of this step point into the synthesized document,
        // not the script; the source name says so.
        let source_name = format!("{} (synthesized manifest)", script_path.display());
        let source = script.synthetic_manifest().map_err(|error| {
            ScriptWorkspaceError::CondaScriptManifest(Box::new(WithSourceCode {
                error: pixi_manifest::TomlError::from(error),
                source: miette::NamedSource::new(&source_name, script.toml().to_owned()),
            }))
        })?;

        let (mut manifest, package, warnings) = TomlManifest::from_toml_str(&source)
            .and_then(|manifest| {
                manifest.into_workspace_manifest(
                    ExternalWorkspaceProperties::default(),
                    PackageDefaults::default(),
                    &root,
                )
            })
            .map_err(|error| {
                ScriptWorkspaceError::CondaScriptManifest(Box::new(WithSourceCode {
                    error,
                    source: miette::NamedSource::new(&source_name, source),
                }))
            })?;
        debug_assert!(
            package.is_none(),
            "a synthetic conda-script manifest never defines a package"
        );

        let cache_root = config
            .cache_dir_for(CacheKind::ExecEnvironments)
            .map_err(|error| ScriptWorkspaceError::CacheDirectory(error.to_string()))?;
        let workspace_script = WorkspaceScript::for_local_conda_script(script, &cache_root);
        let lock_file_path = workspace_script
            .lock_file_path()
            .expect("a local script has an adjacent lock-file path");
        set_implicit_script_platforms(
            &mut manifest.workspace,
            implicit_script_platforms(Some(&lock_file_path))?,
        );

        // The provenance kind only matters for flows that re-read or edit the
        // manifest, which conda-script workspaces reject; PEP 723 is the
        // closest embedded-metadata kind.
        let workspace =
            manifest.with_provenance(ManifestProvenance::new(script_path, ManifestKind::Pep723));

        Ok(WithWarnings::from(Self::from_parsed(
            workspace,
            None,
            root,
            config,
            WorkspaceStorage::Script(workspace_script),
        ))
        .with_warnings(warnings))
    }

    /// Construct an isolated workspace for a transient PEP 723 script.
    pub fn from_transient_script(
        script: ScriptManifest,
        config: Config,
        root: PathBuf,
        provenance_path: PathBuf,
        cache_name: &str,
        cache_key: &[u8],
    ) -> Result<WithWarnings<Self>, ScriptWorkspaceError> {
        let script_manifest = script.clone();
        let script_config = script.workspace_config()?;
        let (mut manifest, warnings) = script.into_workspace_manifest()?;

        if !script_config.channels_explicit {
            manifest.workspace.channels = config
                .default_channels()
                .into_iter()
                .map(PrioritizedChannel::from)
                .collect();
        }
        if !script_config.platforms_explicit {
            // A transient script has nowhere to keep a lock file, so the host
            // is the only platform it can be resolved for.
            set_implicit_script_platforms(
                &mut manifest.workspace,
                implicit_script_platforms(None)?,
            );
        }

        let cache_root = config
            .cache_dir_for(CacheKind::ExecEnvironments)
            .map_err(|error| ScriptWorkspaceError::CacheDirectory(error.to_string()))?;
        let workspace_script = WorkspaceScript::for_transient(
            script_manifest,
            &cache_root,
            cache_name,
            cache_key,
            &root,
        );
        let workspace = manifest.with_provenance(ManifestProvenance::new(
            provenance_path,
            ManifestKind::Pep723,
        ));

        Ok(WithWarnings::from(Self::from_parsed(
            workspace,
            None,
            root,
            config,
            WorkspaceStorage::Script(workspace_script),
        ))
        .with_warnings(warnings))
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
        match &self.storage {
            WorkspaceStorage::Project => self.root.join(consts::PIXI_DIR),
            WorkspaceStorage::Script(script) => script.pixi_dir().to_owned(),
        }
    }

    /// Returns the effective pixi directory for the workspace. When
    /// detached-environments is configured, this returns the project-specific
    /// detached path instead of the default `.pixi` directory.
    pub fn pixi_dir(&self) -> PathBuf {
        if let WorkspaceStorage::Script(script) = &self.storage {
            return script.pixi_dir().to_owned();
        }
        self.detached_environments_path()
            .unwrap_or_else(|| self.default_pixi_dir())
    }

    /// `true` when this is a script that declares no `platforms`, so the
    /// platform it resolves for was picked from the machine rather than from
    /// the script metadata.
    pub fn script_platforms_are_implicit(&self) -> bool {
        let WorkspaceStorage::Script(script) = &self.storage else {
            return false;
        };
        match script.source() {
            ScriptSource::Pep723(manifest) => manifest
                .workspace_config()
                .is_ok_and(|config| !config.platforms_explicit),
            // A conda-script block has no `platforms` key at all.
            ScriptSource::CondaScript(_) => true,
        }
    }

    /// `true` when this workspace was constructed from a conda-script file.
    pub fn is_conda_script(&self) -> bool {
        matches!(
            &self.storage,
            WorkspaceStorage::Script(script)
                if matches!(script.source(), ScriptSource::CondaScript(_))
        )
    }

    /// Create the detached-environments path for this project if it is set in
    /// the config
    fn detached_environments_path(&self) -> Option<PathBuf> {
        if matches!(self.storage, WorkspaceStorage::Script(_)) {
            return None;
        }
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
        match &self.storage {
            WorkspaceStorage::Project => self.root.join(consts::PROJECT_LOCK_FILE),
            WorkspaceStorage::Script(script) => script
                .lock_file_path()
                .expect("transient script workspaces do not have a lock file path"),
        }
    }

    /// Returns the lock file path when this workspace can persist a lock file.
    pub fn persistent_lock_file_path(&self) -> Option<PathBuf> {
        match &self.storage {
            WorkspaceStorage::Project => Some(self.root.join(consts::PROJECT_LOCK_FILE)),
            WorkspaceStorage::Script(script) => script.lock_file_path(),
        }
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

    /// Resolves a conda subdir to the workspace platform that targets it.
    ///
    /// Commands that take a bare subdir on the command line (`pixi publish
    /// --target-platform linux-64`) need the declared platform behind it: the
    /// system requirements that a build depends on (`glibc`, `macos`, `cuda`,
    /// ...) live on the `[workspace] platforms` entries, which may carry a
    /// synthesized name like `linux-64-glibc-2-34`. Reaching for
    /// [`PixiPlatform::from_subdir`] instead picks up pixi's portable defaults
    /// (`__glibc = 2.28`), so the declared requirements silently drop out of
    /// both the derived `c_stdlib`/`c_stdlib_version` build variants and the
    /// virtual packages the build environments solve against
    /// (prefix-dev/pixi#6709).
    ///
    /// Candidates are the declared platforms for `subdir` in manifest order,
    /// preferring one the default environment selects -- which is also the
    /// entry the composition pass registers for the legacy
    /// `[system-requirements]` shape. A subdir the workspace does not declare
    /// (cross-building for `osx-arm64` from a linux-only workspace, say) falls
    /// back to the subdir baseline.
    pub fn pixi_platform_for_subdir(&self, subdir: Platform) -> PixiPlatform {
        let candidates: Vec<&PixiPlatform> = self
            .workspace
            .value
            .workspace
            .platforms
            .iter()
            .filter(|platform| platform.subdir() == subdir)
            .collect();

        let environment_platforms = self.default_environment().platforms();
        candidates
            .iter()
            .find(|platform| environment_platforms.contains(platform.name()))
            .or(candidates.first())
            .map(|platform| (*platform).clone())
            .unwrap_or_else(|| PixiPlatform::from_subdir(subdir))
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

        // Derive `c_stdlib` variants from the platform's system requirements,
        // filling only keys an explicit `[workspace.build-variants]` entry
        // didn't already set -- a hand-written variant always wins. The derived
        // providers are conda-forge packages, so this resolves the workspace's
        // channels and only applies when one of them is conda-forge.
        let channel_config = self.channel_config();
        let manifest = &self.workspace.value;
        let channel_urls: Vec<ChannelUrl> = manifest
            .workspace
            .channels
            .iter()
            .map(|prioritized| &prioritized.channel)
            .chain(
                manifest
                    .all_features()
                    .filter_map(|(_, feature)| feature.channels.as_ref())
                    .flatten()
                    .map(|prioritized| &prioritized.channel),
            )
            .filter_map(|channel| channel.clone().into_base_url(&channel_config).ok())
            .collect();
        for (key, value) in stdlib_variants::derive_stdlib_variants(
            platform,
            &channel_urls,
            stdlib_variants::StdlibVersionPin::Exact,
        ) {
            variant_configuration
                .entry(key)
                .or_insert_with(|| vec![value]);
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
    /// solve / instantiate-backend reporter slots, then lets `progress`
    /// override them with the terminal reporters.
    ///
    /// `progress` is mandatory so that no dispatcher can be constructed
    /// without deciding whether its work is visible to the user. Pass `None`
    /// only for genuinely silent paths; `grep` for it to find them all.
    pub fn command_dispatcher_builder(
        &self,
        progress: Option<&Arc<pixi_reporters::TopLevelProgress>>,
    ) -> miette::Result<CommandDispatcherBuilder> {
        let cache_dir = AbsPathBuf::new(pixi_config::get_cache_dir()?)
            .expect("cache dir is not absolute")
            .into_assume_dir();
        let workspace_dir = AbsPathBuf::new(self.pixi_dir())
            .expect("pixi dir is not absolute")
            .into_assume_dir();
        let cache_dirs = CacheDirs::new(cache_dir).with_workspace(workspace_dir);

        // Determine the tool platform to use
        let tool_platform = self.config().tool_platform();
        let host_subdir = host_subdir();
        let tool_virtual_packages = if tool_platform.only_platform() == host_subdir.only_platform()
        {
            // If the tool platform is the same as the current platform, we just assume the
            // same virtual packages apply.
            host_capabilities()
        } else {
            vec![]
        };

        let root_dir = AbsPathBuf::new(self.root().to_path_buf())
            .expect("root dir is not absolute")
            .into_assume_dir();

        let rayon_primer = std::sync::Arc::new(crate::rayon_primer::RayonPrimer::default());
        let builder = CommandDispatcher::builder()
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
            .with_offline(self.config.offline())
            .with_pixi_install_reporter(rayon_primer.clone())
            .with_pixi_solve_reporter(rayon_primer.clone())
            .with_instantiate_backend_reporter(rayon_primer)
            .with_tool_platform(tool_platform, tool_virtual_packages);

        // Registered last so the terminal reporters win over the primers above.
        Ok(match progress {
            Some(progress) => progress.clone().register_with(builder),
            None => builder,
        })
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

/// A package that `update_dependencies` left untouched in the manifest.
#[derive(Debug, Clone)]
pub struct SkippedPackage {
    /// The normalized package name.
    pub name: String,
    /// True when the manifest entry inherits from `[workspace.dependencies]`
    /// via `{ workspace = true }`.
    pub inherits_workspace: bool,
}

impl<'source> HasWorkspaceManifest<'source> for &'source Workspace {
    fn workspace_manifest(&self) -> &'source WorkspaceManifest {
        &self.workspace.value
    }
}

/// Get or initialize the activated environment variables.
///
/// Note: the result is memoized per environment and behavior, not per
/// platform, so callers activating the same environment with the same
/// behavior must pass the same platform within one process.
pub async fn get_activated_environment_variables<'a>(
    project_env_vars: &'a HashMap<EnvironmentName, EnvironmentVars>,
    environment: &Environment<'_>,
    platform: &PixiPlatform,
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
                        platform,
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
                        platform,
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
                        platform,
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
    use pixi_config::{CacheConfig, Config, DetachedEnvironments};
    use pixi_manifest::{FeatureName, FeaturesExt, HasWorkspaceManifest, script::ScriptManifest};
    use pypi_mapping::{MappingMode, ProjectDefinedChannelMapping, ProjectDefinedMappingLocation};
    use rattler_conda_types::{
        Channel, GenericVirtualPackage, NamedChannelOrUrl, Platform, Version,
    };
    use url::Url;
    use xxhash_rust::xxh3::xxh3_64;

    use super::*;

    /// A platform row carries whatever package names were written into it, and
    /// a long enough one spells out past the platform-name limit. Such a row is
    /// skipped: the rest of the lock still counts, and nothing panics.
    #[test]
    fn an_unnameable_locked_platform_is_skipped() {
        let subdir = host_subdir();
        // `MAX_PLATFORM_NAME_BYTES` is private to `pixi_manifest`, so this is
        // simply longer than any cap that crate would plausibly carry.
        let unnameable = format!("__{}", "a".repeat(1024));
        let lock_source = format!(
            r#"version: 7
platforms:
- name: {subdir}
  subdir: {subdir}
- name: unnameable
  subdir: {subdir}
  virtual-packages:
  - {unnameable}=1
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages: {{}}
packages: []
"#
        );

        let dir = tempfile::tempdir().unwrap();
        let lock_file_path = dir.path().join("script.py.pixi.lock");
        fs_err::write(&lock_file_path, lock_source).unwrap();

        let platforms = implicit_script_platforms(Some(&lock_file_path))
            .expect("an unnameable row must not fail the whole lookup");

        // The bare-subdir row came through, so the lock really was read.
        assert!(
            platforms.iter().any(|p| p.subdir() == subdir),
            "got {:?}",
            platforms.iter().map(|p| p.name().as_str()).collect_vec()
        );
        assert!(
            !platforms.iter().any(|p| p.name().as_str().contains("aaaa")),
            "the unnameable row should have been skipped, got {:?}",
            platforms.iter().map(|p| p.name().as_str()).collect_vec()
        );
    }

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = ["linux-64", "win-64"]
        "#;

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
                .tasks(Some(&osx64), &FeatureName::Default)
                .unwrap()
        );
        assert_debug_snapshot!(
            workspace
                .workspace
                .value
                .tasks(Some(&win64), &FeatureName::Default)
                .unwrap()
        );
        assert_debug_snapshot!(
            workspace
                .workspace
                .value
                .tasks(Some(&linux64), &FeatureName::Default)
                .unwrap()
        );
    }

    /// An explicit `[workspace.build-variants]` entry wins over the value
    /// derived from the platform's system requirements, while a key the user
    /// did not set (`c_stdlib`) is still filled in from the platform.
    #[test]
    fn explicit_build_variant_overrides_derived_stdlib() {
        let file_contents = r#"
            [workspace]
            name = "foo"
            channels = ["conda-forge"]
            platforms = ["osx-arm64"]
            build-variants = { c_stdlib_version = ["99.0"] }
            "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        let platform = pixi_manifest::PixiPlatform::new(
            pixi_manifest::PixiPlatformName::from_str("mac").unwrap(),
            Platform::OsxArm64,
            vec![GenericVirtualPackage {
                name: "__osx".parse().unwrap(),
                version: Version::from_str("13.5").unwrap(),
                build_string: "0".to_string(),
            }],
        )
        .unwrap();

        let variants = workspace.variants(&platform).unwrap().variant_configuration;

        // Explicit override is kept verbatim, not replaced by the derived 13.5.
        assert_eq!(
            variants.get("c_stdlib_version"),
            Some(&vec![VariantValue::String("99.0".to_string())])
        );
        // The provider key the user didn't set is derived from the platform.
        assert_eq!(
            variants.get("c_stdlib"),
            Some(&vec![VariantValue::String(
                "macosx_deployment_target".to_string()
            )])
        );
    }

    /// Reproduces #6566: a custom platform with a patch-level macOS version
    /// (`macos = "15.1.1"` -> `__osx = 15.1.1`) must derive a `major.minor`
    /// `c_stdlib_version`. `macosx_deployment_target_<subdir>` is only published
    /// at `major.minor`, so a `15.1.1` pin resolves to no candidate and the build
    /// solve fails.
    #[test]
    fn osx_patch_version_truncated_to_major_minor() {
        let file_contents = r#"
            [workspace]
            name = "foo"
            channels = ["conda-forge"]
            platforms = ["osx-arm64"]
            "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        let platform = pixi_manifest::PixiPlatform::new(
            pixi_manifest::PixiPlatformName::from_str("my-mac").unwrap(),
            Platform::OsxArm64,
            vec![GenericVirtualPackage {
                name: "__osx".parse().unwrap(),
                version: Version::from_str("15.1.1").unwrap(),
                build_string: "0".to_string(),
            }],
        )
        .unwrap();

        let variants = workspace.variants(&platform).unwrap().variant_configuration;

        assert_eq!(
            variants.get("c_stdlib_version"),
            Some(&vec![VariantValue::String("15.1".to_string())])
        );
    }

    /// Same as [`osx_patch_version_truncated_to_major_minor`] for the linux
    /// `sysroot` provider: `glibc` is likewise only published at `major.minor`,
    /// so a patch-level `__glibc` must be truncated before it becomes a pin.
    #[test]
    fn glibc_patch_version_truncated_to_major_minor() {
        let file_contents = r#"
            [workspace]
            name = "foo"
            channels = ["conda-forge"]
            platforms = ["linux-64"]
            "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        let platform = pixi_manifest::PixiPlatform::new(
            pixi_manifest::PixiPlatformName::from_str("my-linux").unwrap(),
            Platform::Linux64,
            vec![GenericVirtualPackage {
                name: "__glibc".parse().unwrap(),
                version: Version::from_str("2.28.1").unwrap(),
                build_string: "0".to_string(),
            }],
        )
        .unwrap();

        let variants = workspace.variants(&platform).unwrap().variant_configuration;

        assert_eq!(
            variants.get("c_stdlib_version"),
            Some(&vec![VariantValue::String("2.28".to_string())])
        );
    }

    /// Reproduces #6709: a subdir handed to `pixi publish`/`pixi build` must
    /// resolve to the platform the workspace declares for it, so its system
    /// requirements reach the build. Resolving to the subdir baseline instead
    /// would pin `c_stdlib_version` to pixi's default `2.28` and hand the build
    /// solve a `__glibc = 2.28`, making the declared `glibc = "2.34"`
    /// unreachable from a recipe.
    #[test]
    fn subdir_resolves_to_declared_platform() {
        let file_contents = r#"
            [workspace]
            name = "foo"
            channels = ["conda-forge"]
            platforms = [{ platform = "linux-64", glibc = "2.34" }]
            "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        let platform = workspace.pixi_platform_for_subdir(Platform::Linux64);
        assert_eq!(
            platform
                .declared_virtual_packages()
                .iter()
                .find(|vp| vp.name.as_normalized() == "__glibc")
                .map(|vp| vp.version.to_string()),
            Some("2.34".to_string())
        );

        let variants = workspace.variants(&platform).unwrap().variant_configuration;
        assert_eq!(
            variants.get("c_stdlib_version"),
            Some(&vec![VariantValue::String("2.34".to_string())])
        );
    }

    /// A workspace that declares plain subdirs keeps the subdir baseline, and a
    /// subdir it doesn't declare at all (cross-building) falls back to it too.
    #[test]
    fn subdir_without_declared_customisation_uses_baseline() {
        let file_contents = r#"
            [workspace]
            name = "foo"
            channels = ["conda-forge"]
            platforms = ["linux-64"]
            "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), file_contents).unwrap();

        let declared = workspace.pixi_platform_for_subdir(Platform::Linux64);
        assert_eq!(
            declared.declared_virtual_packages(),
            PixiPlatform::from_subdir(Platform::Linux64).declared_virtual_packages()
        );

        let undeclared = workspace.pixi_platform_for_subdir(Platform::OsxArm64);
        assert_eq!(undeclared.name().as_str(), "osx-arm64");
        assert_eq!(
            undeclared.declared_virtual_packages(),
            PixiPlatform::from_subdir(Platform::OsxArm64).declared_virtual_packages()
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
            // A non-conda-forge channel defaults the same-name heuristic off.
            &ProjectDefinedChannelMapping::new(
                vec![ProjectDefinedMappingLocation::Path(
                    workspace
                        .channel_config()
                        .root_dir
                        .join(PathBuf::from("mapping.json"))
                )],
                MappingMode::Overlay,
                false,
            )
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

    fn script_workspace(source: &str, root: &Path, cache: &Path) -> Workspace {
        let path = root.join("example.py");
        fs_err::write(&path, source).unwrap();
        let script = ScriptManifest::from_path(path).unwrap().unwrap();
        Workspace::from_script(
            script,
            Config {
                default_channels: vec![NamedChannelOrUrl::Name("testing".into())],
                cache: CacheConfig {
                    exec_environments: Some(cache.to_owned()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap()
        .value
    }

    #[test]
    fn conda_script_workspace_merges_tool_pixi_into_the_default_environment() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = root.path().join("example.c");
        fs_err::write(
            &path,
            r#"// /// conda-script
// channels = ["testing"]
// entrypoint = "run ${SCRIPT}"
//
// [dependencies]
// zlib = "1.3.*"
//
// [tool.pixi.pypi-dependencies]
// requests = ">=2"
// /// end-conda-script
"#,
        )
        .unwrap();
        let script = CondaScriptManifest::from_path(&path).unwrap().unwrap();

        let workspace = Workspace::from_conda_script(
            script,
            Config {
                cache: CacheConfig {
                    exec_environments: Some(cache.path().to_owned()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap()
        .value;

        let manifest = &workspace.workspace.value;
        assert_eq!(
            manifest
                .workspace
                .channels
                .iter()
                .map(|channel| channel.channel.to_string())
                .collect::<Vec<_>>(),
            ["testing"]
        );
        assert_eq!(manifest.environments.iter().count(), 1);
        assert_eq!(manifest.all_features().count(), 2);
        let default_environment = workspace.default_environment();
        assert!(
            default_environment
                .pypi_dependencies(None)
                .contains_key(&PypiPackageName::from_str("requests").unwrap()),
            "the tool.pixi feature must reach the default environment"
        );
        assert!(
            !manifest.workspace.platforms.is_empty(),
            "a conda-script workspace resolves for the machine it runs on"
        );
        assert!(workspace.script_platforms_are_implicit());
        assert_eq!(
            workspace.lock_file_path(),
            root.path().join("example.c.pixi.lock")
        );
    }

    #[test]
    fn script_workspace_separates_source_state_and_lock_paths() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let workspace = script_workspace(
            r#"# /// script
# dependencies = []
# ///
print("hello")
"#,
            root.path(),
            cache.path(),
        );

        assert_eq!(workspace.root(), root.path());
        assert_eq!(workspace.display_name(), "example");
        assert_eq!(
            workspace.workspace.provenance.path,
            root.path().join("example.py")
        );
        assert_eq!(
            workspace.lock_file_path(),
            root.path().join("example.py.pixi.lock")
        );
        assert!(workspace.default_pixi_dir().starts_with(cache.path()));
        assert!(
            workspace
                .default_pixi_dir()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("example-")
        );
        assert_eq!(workspace.pixi_dir(), workspace.default_pixi_dir());
        assert_eq!(
            workspace.default_environment().dir(),
            workspace
                .default_pixi_dir()
                .join(consts::ENVIRONMENTS_DIR)
                .join("default")
        );
        assert!(!root.path().join(consts::PIXI_DIR).exists());
        assert!(!workspace.lock_file_path().exists());

        let workspace_env = workspace.get_metadata_env();
        assert_eq!(
            workspace_env["PIXI_PROJECT_ROOT"],
            root.path().to_string_lossy()
        );
        assert_eq!(workspace_env["PIXI_PROJECT_NAME"], "example");
        assert_eq!(
            workspace_env["PIXI_PROJECT_MANIFEST"],
            root.path().join("example.py").to_string_lossy()
        );

        let environment_env = workspace.default_environment().get_metadata_env();
        assert_eq!(environment_env["PIXI_ENVIRONMENT_NAME"], "default");

        assert_eq!(
            workspace
                .workspace
                .value
                .workspace
                .channels
                .iter()
                .map(|channel| channel.channel.to_string())
                .collect::<Vec<_>>(),
            ["testing"]
        );
        assert_eq!(
            workspace
                .workspace
                .value
                .workspace
                .platforms
                .iter()
                .map(PixiPlatform::subdir)
                .collect::<Vec<_>>(),
            [Platform::current()]
        );
    }

    #[test]
    fn script_workspace_respects_explicit_empty_defaults() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let workspace = script_workspace(
            r#"# /// script
# dependencies = []
#
# [tool.pixi.workspace]
# channels = []
# platforms = []
#
# [tool.pixi.dependencies]
# ///
"#,
            root.path(),
            cache.path(),
        );

        assert!(workspace.workspace.value.workspace.channels.is_empty());
        assert!(workspace.workspace.value.workspace.platforms.is_empty());
    }

    /// Declaring `platforms` opts a script out of host detection, so it keeps
    /// the bare subdir it asks for instead of this machine's virtual packages.
    #[test]
    fn script_workspace_keeps_explicitly_declared_platforms() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let subdir = host_subdir();
        let workspace = script_workspace(
            &format!(
                r#"# /// script
# dependencies = []
#
# [tool.pixi.workspace]
# channels = []
# platforms = ["{subdir}"]
# ///
"#
            ),
            root.path(),
            cache.path(),
        );

        assert!(!workspace.script_platforms_are_implicit());
        let platforms = &workspace.workspace.value.workspace.platforms;
        assert_eq!(
            platforms.iter().map(PixiPlatform::subdir).collect_vec(),
            [subdir]
        );
        assert!(
            platforms
                .iter()
                .all(|platform| platform.customised_virtual_packages().is_empty()),
            "host detection leaked into an explicitly declared platform: {:?}",
            platforms
        );
    }

    #[test]
    fn script_workspace_cache_identity_includes_the_absolute_path() {
        let first_root = tempfile::tempdir().unwrap();
        let second_root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let source = "# /// script\n# dependencies = []\n# ///\n";

        let first = script_workspace(source, first_root.path(), cache.path());
        let first_again = script_workspace(source, first_root.path(), cache.path());
        let second = script_workspace(source, second_root.path(), cache.path());

        assert_eq!(first.default_pixi_dir(), first_again.default_pixi_dir());
        assert_ne!(first.default_pixi_dir(), second.default_pixi_dir());
    }

    #[test]
    fn script_workspace_drops_foreign_subdirs_from_a_lock_file() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let host = host_subdir();
        // A subdir this machine cannot run. A lock file can hold one when the
        // script declared `platforms` and had the line removed since.
        let foreign = if host.is_windows() {
            Platform::Linux64
        } else {
            Platform::Win64
        };
        let lock_file = LockFile::builder()
            .with_platforms(
                [host, foreign]
                    .into_iter()
                    .map(|subdir| rattler_lock::PlatformData {
                        name: rattler_lock::PlatformName::try_from(subdir.as_str()).unwrap(),
                        subdir,
                        virtual_packages: Vec::new(),
                    })
                    .collect(),
            )
            .unwrap()
            .finish();
        lock_file
            .to_path(&root.path().join("example.py.pixi.lock"))
            .unwrap();

        let workspace = script_workspace(
            "# /// script\n# dependencies = []\n# ///\n",
            root.path(),
            cache.path(),
        );

        assert_eq!(
            workspace
                .workspace
                .value
                .workspace
                .platforms
                .iter()
                .map(PixiPlatform::subdir)
                .collect::<Vec<_>>(),
            [host]
        );
    }

    /// A row carrying nothing beyond the subdir baseline records no machine,
    /// so adopting it would pin the script to pixi's defaults on a machine that
    /// offers more.
    #[test]
    fn a_baseline_locked_platform_gives_way_to_the_host() {
        let subdir = host_subdir();
        let host = detect_host(subdir).unwrap();
        if host.customised_virtual_packages().is_empty() {
            // This machine is itself the baseline, so the recorded row already
            // is the host and there is nothing to prefer over it.
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let lock_file_path = dir.path().join("script.py.pixi.lock");
        fs_err::write(&lock_file_path, baseline_lock_source(subdir)).unwrap();

        assert_eq!(
            implicit_script_platforms(Some(&lock_file_path)).unwrap(),
            IndexSet::from([host])
        );
    }

    /// A row this machine satisfies is reused exactly as recorded, so a
    /// `pixi lock --script` keeps reproducing rather than being re-solved for a
    /// marginally different host.
    #[test]
    fn a_locked_platform_the_host_satisfies_is_reused() {
        let subdir = host_subdir();
        let host = detect_host(subdir).unwrap();
        // One of the machine's own virtual packages: customised, so it is not
        // the baseline, and satisfied, so it survives the capability check.
        let Some(recorded) = host.customised_virtual_packages().first().cloned() else {
            return;
        };
        let build = if recorded.build_string.is_empty() {
            "0"
        } else {
            recorded.build_string.as_str()
        };
        let lock_source = format!(
            r#"version: 7
platforms:
- name: recorded
  subdir: {subdir}
  virtual-packages:
  - {name}={version}={build}
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages: {{}}
packages: []
"#,
            name = recorded.name.as_normalized(),
            version = recorded.version,
        );

        let dir = tempfile::tempdir().unwrap();
        let lock_file_path = dir.path().join("script.py.pixi.lock");
        fs_err::write(&lock_file_path, lock_source).unwrap();

        let platforms = implicit_script_platforms(Some(&lock_file_path)).unwrap();
        assert_eq!(
            platforms
                .iter()
                .map(PixiPlatform::customised_virtual_packages)
                .collect::<Vec<_>>(),
            [vec![recorded]],
        );
    }

    /// A row for this subdir that demands more than the machine offers gives
    /// way to the host, rather than failing on a platform the script never
    /// declared.
    #[test]
    fn a_locked_platform_the_host_cannot_run_gives_way_to_the_host() {
        let subdir = host_subdir();
        let host = detect_host(subdir).unwrap();
        // No machine reports a CUDA driver this new, so the recorded platform
        // is customised (not the baseline) and unsatisfiable.
        let lock_source = format!(
            r#"version: 7
platforms:
- name: recorded
  subdir: {subdir}
  virtual-packages:
  - __cuda=99=0
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages: {{}}
packages: []
"#
        );

        let dir = tempfile::tempdir().unwrap();
        let lock_file_path = dir.path().join("script.py.pixi.lock");
        fs_err::write(&lock_file_path, lock_source).unwrap();

        assert_eq!(
            implicit_script_platforms(Some(&lock_file_path)).unwrap(),
            IndexSet::from([host])
        );
    }

    fn baseline_lock_source(subdir: Platform) -> String {
        format!(
            r#"version: 7
platforms:
- name: {subdir}
  subdir: {subdir}
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages: {{}}
packages: []
"#
        )
    }

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
