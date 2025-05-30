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

pub use error::{CommandDispatcherError, CommandDispatcherErrorResultExt};
pub(crate) use git::GitCheckoutTask;
pub use instantiate_backend::{InstantiateBackendError, InstantiateBackendSpec};
use pixi_build_frontend::BackendOverride;
use pixi_git::resolver::GitResolver;
use pixi_glob::GlobHashCache;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec, PixiRecord};
use pixi_spec::SourceSpec;
use rattler::package_cache::PackageCache;
use rattler_conda_types::prefix::Prefix;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_repodata_gateway::{Gateway, MaxConcurrency};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use reqwest_middleware::ClientWithMiddleware;
use tokio::sync::{mpsc, oneshot};
use typed_path::Utf8TypedPath;

use crate::{
    Executor, InvalidPathError, PixiEnvironmentSpec, SolveCondaEnvironmentSpec,
    SolvePixiEnvironmentError, SourceCheckout, SourceCheckoutError, SourceMetadataSpec,
    cache_dirs::CacheDirs,
    command_dispatcher_processor::CommandDispatcherProcessor,
    install_pixi::{InstallPixiEnvironmentError, InstallPixiEnvironmentSpec},
    instantiate_tool_env::{InstantiateToolEnvironmentError, InstantiateToolEnvironmentSpec},
    limits::{Limits, ResolvedLimits},
    reporter::Reporter,
    source_metadata::{SourceMetadata, SourceMetadataCache, SourceMetadataError},
};

mod error;
mod git;
mod instantiate_backend;

/// The command dispatcher is responsible for synchronizing requests between
/// different conda environments.
#[derive(Clone)]
pub struct CommandDispatcher {
    pub(crate) channel: CommandDispatcherChannel,
    pub(crate) context: Option<CommandDispatcherContext>,
    pub(crate) data: Arc<CommandDispatcherData>,
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

    /// The resolved limits for the command dispatcher.
    pub limits: ResolvedLimits,

    /// The package cache used to store packages.
    pub package_cache: PackageCache,

    /// The platform (and virtual packages) to use for tools that should run on the current system.
    /// Usually this is the current platform, but it can be a different platform.
    pub tool_platform: (Platform, Vec<GenericVirtualPackage>),
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
#[derive(Debug, Copy, Clone, derive_more::From)]
pub(crate) enum CommandDispatcherContext {
    SolveCondaEnvironment(SolveCondaEnvironmentId),
    SolvePixiEnvironment(SolvePixiEnvironmentId),
    SourceMetadata(SourceMetadataId),
    InstallPixiEnvironment(InstallPixiEnvironmentId),
    InstantiateToolEnv(InstantiatedToolEnvId),
}

slotmap::new_key_type! {
    /// An id that uniquely identifies a conda environment that is being solved.
    pub(crate) struct SolveCondaEnvironmentId;

    /// An id that uniquely identifies a conda environment that is being solved.
    pub(crate) struct SolvePixiEnvironmentId;

    /// An id that uniquely identifies an installation of an environment.
    pub(crate) struct InstallPixiEnvironmentId;
}

/// An id that uniquely identifies a source metadata request.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub(crate) struct SourceMetadataId(pub usize);

/// An id that uniquely identifies a tool environment.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub(crate) struct InstantiatedToolEnvId(pub usize);

/// A message send to the dispatch task.
#[derive(derive_more::From)]
pub(crate) enum ForegroundMessage {
    SolveCondaEnvironment(SolveCondaEnvironmentTask),
    SolvePixiEnvironment(SolvePixiEnvironmentTask),
    SourceMetadata(SourceMetadataTask),
    GitCheckout(GitCheckoutTask),
    InstallPixiEnvironment(InstallPixiEnvironmentTask),
    InstantiateToolEnvironment(Task<InstantiateToolEnvironmentSpec>),
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
    type Output = ();
    type Error = InstallPixiEnvironmentError;
}

/// A message that is send to the background task to start solving a particular
/// conda environment.
pub(crate) type SolveCondaEnvironmentTask = Task<SolveCondaEnvironmentSpec>;
impl TaskSpec for SolveCondaEnvironmentSpec {
    type Output = Vec<PixiRecord>;
    type Error = rattler_solve::SolveError;
}

/// A message that is send to the background task to requesting the metadata for
/// a particular source spec.
pub(crate) type SourceMetadataTask = Task<SourceMetadataSpec>;

impl TaskSpec for SourceMetadataSpec {
    type Output = Arc<SourceMetadata>;
    type Error = SourceMetadataError;
}

/// Instantiates a tool environment.
impl TaskSpec for InstantiateToolEnvironmentSpec {
    type Output = Prefix;
    type Error = InstantiateToolEnvironmentError;
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

    /// Returns the cache for source metadata.
    pub fn source_metadata_cache(&self) -> &SourceMetadataCache {
        &self.data.source_metadata_cache
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

    /// Sends a task to the command dispatcher and waits for the result.
    async fn execute_task<T: TaskSpec>(
        &self,
        spec: T,
    ) -> Result<T::Output, CommandDispatcherError<T::Error>>
    where
        ForegroundMessage: From<Task<T>>,
    {
        let Some(sender) = self.channel.sender() else {
            // If this fails, it means the command dispatcher was dropped and the task is
            // immediately canceled.
            return Err(CommandDispatcherError::Cancelled);
        };

        let (tx, rx) = oneshot::channel();
        sender
            .send(ForegroundMessage::from(Task {
                spec,
                parent: self.context,
                tx,
            }))
            .map_err(|_| CommandDispatcherError::Cancelled)?;

        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(err)) => Err(CommandDispatcherError::Failed(err)),
            Err(_) => Err(CommandDispatcherError::Cancelled),
        }
    }

    /// Returns the metadata of the source spec.
    pub async fn source_metadata(
        &self,
        spec: SourceMetadataSpec,
    ) -> Result<Arc<SourceMetadata>, CommandDispatcherError<SourceMetadataError>> {
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
    ) -> Result<(), CommandDispatcherError<InstallPixiEnvironmentError>> {
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
    ) -> Result<Vec<PixiRecord>, CommandDispatcherError<rattler_solve::SolveError>> {
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
    ) -> Result<Prefix, CommandDispatcherError<InstantiateToolEnvironmentError>> {
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
        match source_spec {
            SourceSpec::Url(_) => unimplemented!("fetching URL sources is not yet implemented"),
            SourceSpec::Path(path) => {
                let source_path = self
                    .data
                    .resolve_typed_path(path.path.to_path())
                    .map_err(SourceCheckoutError::from)?;
                Ok(SourceCheckout {
                    path: source_path,
                    pinned: PinnedSourceSpec::Path(PinnedPathSpec { path: path.path }),
                })
            }
            SourceSpec::Git(git_spec) => self.pin_and_checkout_git(git_spec).await,
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
                    .map_err(SourceCheckoutError::from)?;
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

#[derive(Default)]
pub struct CommandDispatcherBuilder {
    gateway: Option<Gateway>,
    root_dir: Option<PathBuf>,
    reporter: Option<Box<dyn Reporter>>,
    git_resolver: Option<GitResolver>,
    download_client: Option<ClientWithMiddleware>,
    cache_dirs: Option<CacheDirs>,
    build_backend_overrides: BackendOverride,
    max_download_concurrency: MaxConcurrency,
    limits: Limits,
    executor: Executor,
    tool_platform: Option<(Platform, Vec<GenericVirtualPackage>)>,
}

impl CommandDispatcherBuilder {
    /// Sets the cache directories to use.
    pub fn with_cache_dirs(self, cache_dirs: CacheDirs) -> Self {
        Self {
            cache_dirs: Some(cache_dirs),
            ..self
        }
    }

    /// Sets the gateway to use for querying conda repodata.
    pub fn with_gateway(self, gateway: Gateway) -> Self {
        Self {
            gateway: Some(gateway),
            ..self
        }
    }

    /// Sets the reporter used by the [`CommandDispatcher`] to report progress.
    pub fn with_reporter<F: Reporter + 'static>(self, reporter: F) -> Self {
        Self {
            reporter: Some(Box::new(reporter)),
            ..self
        }
    }

    /// Sets the reqwest client to use for network fetches.
    pub fn with_download_client(self, client: ClientWithMiddleware) -> Self {
        Self {
            download_client: Some(client),
            ..self
        }
    }

    /// Sets the git resolver used to fetch git repositories.
    pub fn with_git_resolver(self, resolver: GitResolver) -> Self {
        Self {
            git_resolver: Some(resolver),
            ..self
        }
    }

    /// Sets the root directory for resolving relative paths.
    pub fn with_root_dir(self, root_dir: PathBuf) -> Self {
        Self {
            root_dir: Some(root_dir),
            ..self
        }
    }

    /// Apply overrides to particular backends.
    pub fn with_backend_overrides(self, overrides: BackendOverride) -> Self {
        Self {
            build_backend_overrides: overrides,
            ..self
        }
    }

    /// Sets the maximum number of concurrent downloads.
    pub fn with_max_download_concurrency(self, max_concurrency: impl Into<MaxConcurrency>) -> Self {
        Self {
            max_download_concurrency: max_concurrency.into(),
            ..self
        }
    }

    /// Sets the tool platform and virtual packages associated with it. This is used when
    /// instantiating tool environments and defaults to the current platform.
    pub fn with_tool_platform(
        self,
        platform: Platform,
        virtual_packages: Vec<GenericVirtualPackage>,
    ) -> Self {
        Self {
            tool_platform: Some((platform, virtual_packages)),
            ..self
        }
    }

    /// Set the limits to which this instance should adhere.
    pub fn with_limits(self, limits: Limits) -> Self {
        Self { limits, ..self }
    }

    /// Sets the executor to use for the command dispatcher.
    pub fn with_executor(self, executor: Executor) -> Self {
        Self { executor, ..self }
    }

    /// Completes the builder and returns a new [`CommandDispatcher`].
    pub fn finish(self) -> CommandDispatcher {
        let root_dir = self
            .root_dir
            .or(std::env::current_dir().ok())
            .unwrap_or_default();
        let cache_dirs = self
            .cache_dirs
            .unwrap_or_else(|| CacheDirs::new(root_dir.join(".cache")));
        let download_client = self.download_client.unwrap_or_default();
        let package_cache = PackageCache::new(cache_dirs.packages());
        let gateway = self.gateway.unwrap_or_else(|| {
            Gateway::builder()
                .with_client(download_client.clone())
                .with_cache_dir(cache_dirs.root().clone())
                .with_package_cache(package_cache.clone())
                .with_max_concurrent_requests(self.max_download_concurrency)
                .finish()
        });

        let git_resolver = self.git_resolver.unwrap_or_default();
        let source_metadata_cache = SourceMetadataCache::new(cache_dirs.source_metadata());
        let tool_platform = self.tool_platform.unwrap_or_else(|| {
            let platform = Platform::current();
            let virtual_packages =
                VirtualPackages::detect(&VirtualPackageOverrides::default()).unwrap_or_default();
            (
                platform,
                virtual_packages.into_generic_virtual_packages().collect(),
            )
        });

        let data = Arc::new(CommandDispatcherData {
            gateway,
            source_metadata_cache,
            root_dir,
            git_resolver,
            cache_dirs,
            download_client,
            build_backend_overrides: self.build_backend_overrides,
            glob_hash_cache: GlobHashCache::default(),
            limits: ResolvedLimits::from(self.limits),
            package_cache,
            tool_platform,
        });

        let sender = CommandDispatcherProcessor::spawn(data.clone(), self.reporter, self.executor);
        CommandDispatcher {
            channel: CommandDispatcherChannel::Strong(sender),
            context: None,
            data,
        }
    }
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
}
