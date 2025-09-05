//! Defines the [`CommandDispatcher`] and its associated components for managing
//! and synchronizing tasks across different environments.
//!
//! The [`CommandDispatcher`] is a central component for orchestrating tasks
//! such as solving environments, fetching metadata, and managing source
//! checkouts. It ensures efficient execution by avoiding redundant computations
//! and supporting concurrent operations.

use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub use builder::CommandDispatcherBuilder;
pub use error::{CommandDispatcherError, CommandDispatcherErrorResultExt};
pub(crate) use git::GitCheckoutTask;
pub use instantiate_backend::{InstantiateBackendError, InstantiateBackendSpec};
use pixi_build_discovery::{DiscoveredBackend, EnabledProtocols};
use pixi_build_frontend::BackendOverride;
use pixi_git::resolver::GitResolver;
use pixi_glob::GlobHashCache;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec, PixiRecord};
use pixi_spec::{SourceLocationSpec, SourceSpec};
use rattler::package_cache::PackageCache;
use rattler_conda_types::{ChannelConfig, GenericVirtualPackage, Platform};
use rattler_repodata_gateway::Gateway;
use reqwest_middleware::ClientWithMiddleware;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use typed_path::Utf8TypedPath;

use crate::{
    BuildBackendMetadata, BuildBackendMetadataError, BuildBackendMetadataSpec, Executor,
    InvalidPathError, PixiEnvironmentSpec, SolveCondaEnvironmentSpec, SolvePixiEnvironmentError,
    SourceBuildCacheEntry, SourceBuildCacheStatusError, SourceBuildCacheStatusSpec, SourceCheckout,
    SourceCheckoutError, SourceMetadata, SourceMetadataError, SourceMetadataSpec,
    backend_source_build::{BackendBuiltSource, BackendSourceBuildError, BackendSourceBuildSpec},
    build::{BuildCache, source_metadata_cache::SourceMetadataCache},
    cache_dirs::CacheDirs,
    discover_backend_cache::DiscoveryCache,
    install_pixi::{
        InstallPixiEnvironmentError, InstallPixiEnvironmentResult, InstallPixiEnvironmentSpec,
    },
    instantiate_tool_env::{
        InstantiateToolEnvironmentError, InstantiateToolEnvironmentResult,
        InstantiateToolEnvironmentSpec,
    },
    limits::ResolvedLimits,
    solve_conda::SolveCondaEnvironmentError,
    source_build::{SourceBuildError, SourceBuildResult, SourceBuildSpec},
};

mod builder;
mod error;
mod git;
mod instantiate_backend;

/// The command dispatcher is responsible for synchronizing requests between
/// different conda environments.
#[derive(Clone)]
pub struct CommandDispatcher {
    /// The channel through which messages are sent to the command dispatcher.
    ///
    /// This is an option so we can drop this field in the `Drop`
    /// implementation. It should only ever be `None` when the command
    /// dispatcher is dropped.
    pub(crate) channel: Option<CommandDispatcherChannel>,

    /// The context in which the command dispatcher is operating. If a command
    /// dispatcher is created for a background task, this context will indicate
    /// from which task it was created.
    pub(crate) context: Option<CommandDispatcherContext>,

    /// Holds the shared data required by the command dispatcher.
    pub(crate) data: Arc<CommandDispatcherData>,

    /// Holds a strong reference to the process thread handle, this allows us to
    /// wait for the background thread to finish once the last (user facing)
    /// command dispatcher is dropped.
    pub(crate) processor_handle: Option<Arc<std::thread::JoinHandle<()>>>,
}

impl Drop for CommandDispatcher {
    fn drop(&mut self) {
        // Release our strong reference to the main thread channel. If this is the last
        // strong reference to the channel, the thread will shut down.
        drop(self.channel.take());

        // If this instance holds the last strong reference to the background thread
        // join handle this will await its shutdown.
        if let Some(handle) = self.processor_handle.take().and_then(Arc::into_inner) {
            let _err = handle.join();
        }
    }
}

/// Contains shared data required by the [`CommandDispatcher`].
///
/// This struct holds various components such as the gateway for querying
/// repodata, cache directories, and network clients.
pub(crate) struct CommandDispatcherData {
    /// The gateway to use to query conda repodata.
    pub gateway: Gateway,

    /// Source metadata cache used to store metadata for source packages.
    pub source_metadata_cache: SourceMetadataCache,

    /// Build cache used to store build artifacts for source packages.
    pub build_cache: BuildCache,

    /// The resolver of git repositories.
    pub git_resolver: GitResolver,

    /// The base directory to use if relative paths are discovered.
    pub root_dir: PathBuf,

    /// The location to store caches.
    pub cache_dirs: CacheDirs,

    /// The reqwest client to use for network requests.
    pub download_client: ClientWithMiddleware,

    /// Backend overrides for build environments.
    pub build_backend_overrides: BackendOverride,

    /// A cache for glob hashes.
    pub glob_hash_cache: GlobHashCache,

    /// Cache for discovered build backends keyed by source checkout path.
    pub discovery_cache: DiscoveryCache,

    /// The resolved limits for the command dispatcher.
    pub limits: ResolvedLimits,

    /// The package cache used to store packages.
    pub package_cache: PackageCache,

    /// The platform (and virtual packages) to use for tools that should run on
    /// the current system. Usually this is the current platform, but it can
    /// be a different platform.
    pub tool_platform: (Platform, Vec<GenericVirtualPackage>),

    /// True if execution of link scripts is enabled.
    pub execute_link_scripts: bool,

    /// The execution type of the dispatcher.
    pub executor: Executor,
}

/// A channel through which to send any messages to the command_dispatcher. Some
/// dispatchers are constructed by the command_dispatcher itself. To avoid a
/// cyclic dependency, these "sub"-dispatchers use a weak reference to the
/// sender.
#[derive(Clone)]
pub(crate) enum CommandDispatcherChannel {
    Strong(mpsc::UnboundedSender<ForegroundMessage>),
    Weak(mpsc::WeakUnboundedSender<ForegroundMessage>),
}

impl CommandDispatcherChannel {
    /// Returns an owned channel that can be used to send messages to the
    /// background task, or `None` if the background task has been dropped.
    pub fn sender(&self) -> Option<mpsc::UnboundedSender<ForegroundMessage>> {
        match self {
            CommandDispatcherChannel::Strong(sender) => Some(sender.clone()),
            CommandDispatcherChannel::Weak(sender) => sender.upgrade(),
        }
    }
}

/// Context in which the [`CommandDispatcher`] is operating.
///
/// This enum is used to track dependencies and associate tasks with specific
/// contexts.
#[derive(Debug, Copy, Clone, derive_more::From, derive_more::TryInto, Hash, Eq, PartialEq)]
pub(crate) enum CommandDispatcherContext {
    SolveCondaEnvironment(SolveCondaEnvironmentId),
    SolvePixiEnvironment(SolvePixiEnvironmentId),
    BuildBackendMetadata(BuildBackendMetadataId),
    BackendSourceBuild(BackendSourceBuildId),
    SourceMetadata(SourceMetadataId),
    SourceBuild(SourceBuildId),
    QuerySourceBuildCache(SourceBuildCacheStatusId),
    InstallPixiEnvironment(InstallPixiEnvironmentId),
    InstantiateToolEnv(InstantiatedToolEnvId),
}

slotmap::new_key_type! {
    /// An id that uniquely identifies a conda environment that is being solved.
    pub(crate) struct SolveCondaEnvironmentId;

    /// An id that uniquely identifies a build backend source build request.
    pub(crate) struct BackendSourceBuildId;

    /// An id that uniquely identifies a conda environment that is being solved.
    pub(crate) struct SolvePixiEnvironmentId;

    /// An id that uniquely identifies an installation of an environment.
    pub(crate) struct InstallPixiEnvironmentId;

    /// A unique id that identifies a git source checkout.
    pub(crate) struct GitCheckoutId;
}

/// An id that uniquely identifies a build backend metadata request.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub(crate) struct BuildBackendMetadataId(pub usize);

/// An id that uniquely identifies a source metadata request.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub(crate) struct SourceMetadataId(pub usize);

/// An id that uniquely identifies a source build request.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub(crate) struct SourceBuildId(pub usize);

/// An id that uniquely identifies a source build cache request.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub(crate) struct SourceBuildCacheStatusId(pub usize);

/// An id that uniquely identifies a tool environment.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub(crate) struct InstantiatedToolEnvId(pub usize);

/// A message send to the dispatch task.
#[derive(derive_more::From)]
pub(crate) enum ForegroundMessage {
    SolveCondaEnvironment(SolveCondaEnvironmentTask),
    SolvePixiEnvironment(SolvePixiEnvironmentTask),
    BuildBackendMetadata(BuildBackendMetadataTask),
    BackendSourceBuild(BackendSourceBuildTask),
    SourceMetadata(SourceMetadataTask),
    SourceBuild(SourceBuildTask),
    QuerySourceBuildCache(SourceBuildCacheStatusTask),
    GitCheckout(GitCheckoutTask),
    InstallPixiEnvironment(InstallPixiEnvironmentTask),
    InstantiateToolEnvironment(Task<InstantiateToolEnvironmentSpec>),
    ClearReporter(oneshot::Sender<()>),
}

/// A message that is send to the background task to start solving a particular
/// pixi environment.
pub(crate) type SolvePixiEnvironmentTask = Task<PixiEnvironmentSpec>;
impl TaskSpec for PixiEnvironmentSpec {
    type Output = Vec<PixiRecord>;
    type Error = SolvePixiEnvironmentError;
}

/// A message that is send to the background task to install a particular
/// pixi environment.
pub(crate) type InstallPixiEnvironmentTask = Task<InstallPixiEnvironmentSpec>;
impl TaskSpec for InstallPixiEnvironmentSpec {
    type Output = InstallPixiEnvironmentResult;
    type Error = InstallPixiEnvironmentError;
}

/// A message that is send to the background task to start solving a particular
/// conda environment.
pub(crate) type SolveCondaEnvironmentTask = Task<SolveCondaEnvironmentSpec>;
impl TaskSpec for SolveCondaEnvironmentSpec {
    type Output = Vec<PixiRecord>;
    type Error = SolveCondaEnvironmentError;
}

/// A message that is send to the background task to requesting the metadata for
/// a particular source spec.
pub(crate) type BuildBackendMetadataTask = Task<BuildBackendMetadataSpec>;

impl TaskSpec for BuildBackendMetadataSpec {
    type Output = Arc<BuildBackendMetadata>;
    type Error = BuildBackendMetadataError;
}

pub(crate) type SourceMetadataTask = Task<SourceMetadataSpec>;
impl TaskSpec for SourceMetadataSpec {
    type Output = Arc<SourceMetadata>;
    type Error = SourceMetadataError;
}

pub(crate) type SourceBuildTask = Task<SourceBuildSpec>;

impl TaskSpec for SourceBuildSpec {
    type Output = SourceBuildResult;
    type Error = SourceBuildError;
}

pub(crate) type BackendSourceBuildTask = Task<BackendSourceBuildSpec>;

impl TaskSpec for BackendSourceBuildSpec {
    type Output = BackendBuiltSource;
    type Error = BackendSourceBuildError;
}

/// Instantiates a tool environment.
impl TaskSpec for InstantiateToolEnvironmentSpec {
    type Output = InstantiateToolEnvironmentResult;
    type Error = InstantiateToolEnvironmentError;
}

pub(crate) type SourceBuildCacheStatusTask = Task<SourceBuildCacheStatusSpec>;

impl TaskSpec for SourceBuildCacheStatusSpec {
    type Output = Arc<SourceBuildCacheEntry>;
    type Error = SourceBuildCacheStatusError;
}

impl Default for CommandDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandDispatcher {
    /// Constructs a new default constructed instance.
    pub fn new() -> Self {
        Self::builder().finish()
    }

    /// Constructs a new builder for the command dispatcher.
    pub fn builder() -> CommandDispatcherBuilder {
        CommandDispatcherBuilder::default()
    }

    /// Returns the executor used by the command dispatcher.
    pub fn executor(&self) -> Executor {
        self.data.executor
    }

    /// Returns the cache for source metadata.
    pub fn source_metadata_cache(&self) -> &SourceMetadataCache {
        &self.data.source_metadata_cache
    }

    /// Returns the build cache for source packages.
    pub fn build_cache(&self) -> &BuildCache {
        &self.data.build_cache
    }

    /// Returns the gateway used to query conda repodata.
    pub fn gateway(&self) -> &Gateway {
        &self.data.gateway
    }

    /// Returns any build backend overrides.
    pub fn build_backend_overrides(&self) -> &BackendOverride {
        &self.data.build_backend_overrides
    }

    /// Returns the cache directories used by the command dispatcher.
    pub fn cache_dirs(&self) -> &CacheDirs {
        &self.data.cache_dirs
    }

    /// Returns the glob hash cache.
    pub fn glob_hash_cache(&self) -> &GlobHashCache {
        &self.data.glob_hash_cache
    }

    /// Returns the discovery cache for build backends.
    pub fn discovery_cache(&self) -> &DiscoveryCache {
        &self.data.discovery_cache
    }

    /// Returns the download client used by the command dispatcher.
    pub fn download_client(&self) -> &ClientWithMiddleware {
        &self.data.download_client
    }

    /// Returns the package cache used by the command dispatcher.
    pub fn package_cache(&self) -> &PackageCache {
        &self.data.package_cache
    }

    /// Returns the platform and virtual packages used for tool environments.
    pub fn tool_platform(&self) -> (Platform, &[GenericVirtualPackage]) {
        (self.data.tool_platform.0, &self.data.tool_platform.1)
    }

    /// Returns true if execution of link scripts is enabled.
    pub fn allow_execute_link_scripts(&self) -> bool {
        self.data.execute_link_scripts
    }

    /// Returns the channel used to send messages to the command dispatcher.
    fn channel(&self) -> &CommandDispatcherChannel {
        self.channel
            .as_ref()
            .expect("command dispatcher has been dropped")
    }

    /// Sends a task to the command dispatcher and waits for the result.
    async fn execute_task<T: TaskSpec>(
        &self,
        spec: T,
    ) -> Result<T::Output, CommandDispatcherError<T::Error>>
    where
        ForegroundMessage: From<Task<T>>,
    {
        let Some(sender) = self.channel().sender() else {
            // If this fails, it means the command dispatcher was dropped and the task is
            // immediately canceled.
            return Err(CommandDispatcherError::Cancelled);
        };

        let cancellation_token = CancellationToken::new();
        let (tx, rx) = oneshot::channel();
        sender
            .send(ForegroundMessage::from(Task {
                spec,
                parent: self.context,
                tx,
                cancellation_token: cancellation_token.clone(),
            }))
            .map_err(|_| CommandDispatcherError::Cancelled)?;

        // Make sure to trigger the cancellation token when this async task is dropped.
        let _cancel_guard = cancellation_token.drop_guard();

        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(err)) => Err(CommandDispatcherError::Failed(err)),
            Err(_) => Err(CommandDispatcherError::Cancelled),
        }
    }

    /// Notifies the progress reporter that it should clear its output.
    pub async fn clear_reporter(&self) {
        let Some(sender) = self.channel().sender() else {
            // If this fails, it means the command dispatcher was dropped and the task is
            // immediately canceled.
            return;
        };
        let (tx, rx) = oneshot::channel();
        let _ = sender.send(ForegroundMessage::ClearReporter(tx));
        let _ = rx.await;
    }

    /// Returns the metadata of the source spec.
    pub async fn build_backend_metadata(
        &self,
        spec: BuildBackendMetadataSpec,
    ) -> Result<Arc<BuildBackendMetadata>, CommandDispatcherError<BuildBackendMetadataError>> {
        self.execute_task(spec).await
    }

    /// Returns the metadata of a particular source package.
    pub async fn source_metadata(
        &self,
        spec: SourceMetadataSpec,
    ) -> Result<Arc<SourceMetadata>, CommandDispatcherError<SourceMetadataError>> {
        self.execute_task(spec).await
    }

    /// Query the source build cache for a particular source package.
    pub async fn source_build_cache_status(
        &self,
        spec: SourceBuildCacheStatusSpec,
    ) -> Result<Arc<SourceBuildCacheEntry>, CommandDispatcherError<SourceBuildCacheStatusError>>
    {
        self.execute_task(spec).await
    }

    /// Builds the source package and returns the built conda package.
    pub async fn source_build(
        &self,
        spec: SourceBuildSpec,
    ) -> Result<SourceBuildResult, CommandDispatcherError<SourceBuildError>> {
        self.execute_task(spec).await
    }

    /// Calls into a pixi build backend to perform a source build.
    pub(crate) async fn backend_source_build(
        &self,
        spec: BackendSourceBuildSpec,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>> {
        self.execute_task(spec).await
    }

    /// Solves a particular pixi environment specified by `PixiEnvironmentSpec`.
    ///
    /// This function processes all package requirements defined in the spec,
    /// handling both binary and source packages. For source packages, it:
    ///
    /// 1. Checks out source code repositories
    /// 2. Builds necessary environments for processing source dependencies
    /// 3. Queries metadata from source packages
    /// 4. Recursively processes any transitive source dependencies
    ///
    /// The function automatically deduplicates work when the same source is
    /// referenced multiple times, and ensures efficient parallel execution
    /// where possible.
    pub async fn solve_pixi_environment(
        &self,
        spec: PixiEnvironmentSpec,
    ) -> Result<Vec<PixiRecord>, CommandDispatcherError<SolvePixiEnvironmentError>> {
        self.execute_task(spec).await
    }

    /// Install a pixi environment.
    ///
    /// This method takes a previously solved environment specification and
    /// installs all required packages into the target prefix. It handles
    /// both binary packages (from conda repositories) and source packages
    /// (built from source code).
    pub async fn install_pixi_environment(
        &self,
        spec: InstallPixiEnvironmentSpec,
    ) -> Result<InstallPixiEnvironmentResult, CommandDispatcherError<InstallPixiEnvironmentError>>
    {
        self.execute_task(spec).await
    }

    /// Solves a particular conda environment.
    ///
    /// This method processes a complete environment specification containing
    /// both binary and source packages to find a compatible set of packages
    /// that satisfy all requirements and constraints.
    ///
    /// Unlike solving pixi environments, this method does not perform recursive
    /// source resolution and querying repodata as all information is already
    /// available in the specification.
    pub async fn solve_conda_environment(
        &self,
        spec: SolveCondaEnvironmentSpec,
    ) -> Result<Vec<PixiRecord>, CommandDispatcherError<SolveCondaEnvironmentError>> {
        self.execute_task(spec).await
    }

    /// Instantiates an environment for a tool based on the given spec. Reuses
    /// the environment if possible.
    ///
    /// This method creates isolated environments for build backends and other
    /// tools. These environments are specialized containers with specific
    /// packages needed for particular tasks like building packages,
    /// extracting metadata, or running tools.
    pub async fn instantiate_tool_environment(
        &self,
        spec: InstantiateToolEnvironmentSpec,
    ) -> Result<
        InstantiateToolEnvironmentResult,
        CommandDispatcherError<InstantiateToolEnvironmentError>,
    > {
        self.execute_task(spec).await
    }

    /// Checks out a particular source based on a source spec.
    ///
    /// This function resolves the source specification to a concrete checkout
    /// by:
    /// 1. For path sources: Resolving relative paths against the root directory
    /// 2. For git sources: Cloning or fetching the repository and checking out
    ///    the specified reference
    /// 3. For URL sources: Downloading and extracting the archive (currently
    ///    unimplemented)
    ///
    /// The function handles path normalization and ensures security by
    /// preventing directory traversal attacks. It also manages caching of
    /// source checkouts to avoid redundant downloads or clones when the
    /// same source is used multiple times.
    pub async fn pin_and_checkout(
        &self,
        source_spec: SourceSpec,
    ) -> Result<SourceCheckout, CommandDispatcherError<SourceCheckoutError>> {
        match source_spec.location {
            SourceLocationSpec::Url(url) => {
                unimplemented!("fetching URL sources ({}) is not yet implemented", url.url)
            }
            SourceLocationSpec::Path(path) => {
                let source_path = self
                    .data
                    .resolve_typed_path(path.path.to_path())
                    .map_err(SourceCheckoutError::from)
                    .map_err(CommandDispatcherError::Failed)?;
                Ok(SourceCheckout {
                    path: source_path,
                    pinned: PinnedSourceSpec::Path(PinnedPathSpec { path: path.path }),
                })
            }
            SourceLocationSpec::Git(git_spec) => self.pin_and_checkout_git(git_spec).await,
        }
    }

    /// Checkout pinned source record.
    ///
    /// Similar to `pin_and_checkout` but works with already pinned source
    /// specifications. This is used when we have a concrete revision (e.g.,
    /// a specific git commit) that we want to check out rather than
    /// resolving a reference like a branch name.
    ///
    /// The method handles different source types appropriately:
    /// - For path sources: Resolves and validates the path
    /// - For git sources: Checks out the specific revision
    /// - For URL sources: Extracts the archive with the exact checksum
    ///   (unimplemented)
    pub async fn checkout_pinned_source(
        &self,
        pinned_spec: PinnedSourceSpec,
    ) -> Result<SourceCheckout, CommandDispatcherError<SourceCheckoutError>> {
        match pinned_spec {
            PinnedSourceSpec::Path(ref path) => {
                let source_path = self
                    .data
                    .resolve_typed_path(path.path.to_path())
                    .map_err(SourceCheckoutError::from)
                    .map_err(CommandDispatcherError::Failed)?;
                Ok(SourceCheckout {
                    path: source_path,
                    pinned: pinned_spec,
                })
            }
            PinnedSourceSpec::Git(git_spec) => self.checkout_pinned_git(git_spec).await,
            PinnedSourceSpec::Url(_) => {
                unimplemented!("fetching URL sources is not yet implemented")
            }
        }
    }

    /// Discovers the build backend at a specific path on disk and caches it by
    /// path.
    pub async fn discover_backend(
        &self,
        source_path: &std::path::Path,
        channel_config: ChannelConfig,
        enabled_protocols: EnabledProtocols,
    ) -> Result<Arc<DiscoveredBackend>, CommandDispatcherError<pixi_build_discovery::DiscoveryError>>
    {
        self.discovery_cache()
            .get_or_discover(source_path, &channel_config, &enabled_protocols)
            .await
    }
}

impl CommandDispatcherData {
    /// Resolves the source path to a full path.
    ///
    /// This function does not check if the path exists and also does not follow
    /// symlinks.
    fn resolve_typed_path(&self, path_spec: Utf8TypedPath) -> Result<PathBuf, InvalidPathError> {
        if path_spec.is_absolute() {
            Ok(Path::new(path_spec.as_str()).to_path_buf())
        } else if let Ok(user_path) = path_spec.strip_prefix("~/") {
            let home_dir = dirs::home_dir().ok_or_else(|| {
                InvalidPathError::CouldNotDetermineHomeDirectory(PathBuf::from(path_spec.as_str()))
            })?;
            debug_assert!(home_dir.is_absolute());
            normalize_absolute_path(&home_dir.join(Path::new(user_path.as_str())))
        } else {
            let root_dir = self.root_dir.as_path();
            let native_path = Path::new(path_spec.as_str());
            debug_assert!(root_dir.is_absolute());
            normalize_absolute_path(&root_dir.join(native_path))
        }
    }
}

/// Normalize a path, removing things like `.` and `..`.
///
/// Source: <https://github.com/rust-lang/cargo/blob/b48c41aedbd69ee3990d62a0e2006edbb506a480/crates/cargo-util/src/paths.rs#L76C1-L109C2>
fn normalize_absolute_path(path: &Path) -> Result<PathBuf, InvalidPathError> {
    let mut components = path.components().peekable();
    let mut ret = if let Some(c @ Component::Prefix(..)) = components.peek().copied() {
        components.next();
        PathBuf::from(c.as_os_str())
    } else {
        PathBuf::new()
    };

    for component in components {
        match component {
            Component::Prefix(..) => unreachable!(),
            Component::RootDir => {
                ret.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !ret.pop() {
                    return Err(InvalidPathError::RelativePathEscapesRoot(
                        path.to_path_buf(),
                    ));
                }
            }
            Component::Normal(c) => {
                ret.push(c);
            }
        }
    }
    Ok(ret)
}

/// Defines the inputs and outputs of a certain foreground task specification.
pub(crate) trait TaskSpec {
    type Output;
    type Error;
}

pub(crate) struct Task<S: TaskSpec> {
    pub spec: S,
    pub parent: Option<CommandDispatcherContext>,
    pub tx: oneshot::Sender<Result<S::Output, S::Error>>,
    pub cancellation_token: CancellationToken,
}
