use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use git::GitCheckoutTask;
use pixi_build_frontend::BackendOverride;
use pixi_git::resolver::GitResolver;
use pixi_glob::GlobHashCache;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec, PixiRecord};
use pixi_spec::SourceSpec;
use processor::CommandQueueProcessor;
use rattler_conda_types::RepoDataRecord;
use rattler_repodata_gateway::Gateway;
use reqwest_middleware::ClientWithMiddleware;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use typed_path::Utf8TypedPath;

use crate::{
    CondaEnvironmentSpec, InvalidPathError, PixiEnvironmentSpec, SolveCondaEnvironmentError,
    SolvePixiEnvironmentError, SourceCheckout, SourceCheckoutError, SourceMetadataSpec,
    cache_dirs::CacheDirs,
    reporter::Reporter,
    source_metadata::{SourceMetadata, SourceMetadataError},
};

mod git;
mod instantiate_backend;
mod processor;

/// The command_queue is responsible for synchronizing requests between
/// different conda environments.
#[derive(Clone)]
pub struct CommandQueue {
    channel: CommandQueueChannel,
    context: Option<CommandQueueContext>,
    data: Arc<CommandQueueData>,
}

struct CommandQueueData {
    /// The gateway to use to query conda repodata.
    gateway: Gateway,

    /// The resolver of git repositories
    git_resolver: GitResolver,

    /// The base directory to use if relative paths are discovered.
    root_dir: PathBuf,

    /// The location to store caches
    cache_dirs: CacheDirs,

    /// The reqwest client to use for network requests
    client: ClientWithMiddleware,

    /// Backend overrides
    build_backend_overrides: BackendOverride,

    /// A cache for hashes
    glob_hash_cache: GlobHashCache,
}

/// A channel through which to send any messages to the command_queue. Some
/// dispatchers are constructed by the command_queue itself. To avoid a
/// cyclic dependency, these "sub"-dispatchers use a weak reference to the
/// sender.
#[derive(Clone)]
enum CommandQueueChannel {
    Strong(mpsc::UnboundedSender<ForegroundMessage>),
    Weak(mpsc::WeakUnboundedSender<ForegroundMessage>),
}

impl CommandQueueChannel {
    /// Returns an owned channel that can be used to send messages to the
    /// background task, or `None` if the background task has been dropped.
    pub fn sender(&self) -> Option<mpsc::UnboundedSender<ForegroundMessage>> {
        match self {
            CommandQueueChannel::Strong(sender) => Some(sender.clone()),
            CommandQueueChannel::Weak(sender) => sender.upgrade(),
        }
    }
}

/// The context in which this particular command_queue is running. This is used
/// to track dependencies.
#[derive(Debug, Copy, Clone)]
enum CommandQueueContext {
    SolveCondaEnvironment(SolveCondaEnvironmentId),
    SolvePixiEnvironment(SolvePixiEnvironmentId),
    SourceMetadata(SourceMetadataId),
}

slotmap::new_key_type! {
    /// An id that uniquely identifies a conda environment that is being solved.
    struct SolveCondaEnvironmentId;

    /// An id that uniquely identifies a conda environment that is being solved.
    struct SolvePixiEnvironmentId;
}


/// An id that uniquely identifies a source metadata request.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
struct SourceMetadataId(usize);

/// Wraps an error that might have occurred during the processing of a task.
#[derive(Debug, Clone, Error)]
pub enum CommandQueueError<E> {
    Cancelled,

    #[error(transparent)]
    Failed(#[from] E),
}

impl<E> CommandQueueError<E> {
    pub fn map<U, F: FnOnce(E) -> U>(self, map: F) -> CommandQueueError<U> {
        match self {
            CommandQueueError::Cancelled => CommandQueueError::Cancelled,
            CommandQueueError::Failed(err) => CommandQueueError::Failed(map(err)),
        }
    }

    pub fn into_failed(self) -> Option<E> {
        match self {
            CommandQueueError::Cancelled => None,
            CommandQueueError::Failed(err) => Some(err),
        }
    }
}

/// Convenience trait to make working with `CommandQueueError` type easier.
pub(crate) trait CommandQueueErrorResultExt<T, E> {
    fn map_err_with<U, F: FnOnce(E) -> U>(self, fun: F) -> Result<T, CommandQueueError<U>>;

    fn into_ok_or_failed(self) -> Option<Result<T, E>>;
}

impl<T, E> CommandQueueErrorResultExt<T, E> for Result<T, CommandQueueError<E>> {
    fn map_err_with<U, F: FnOnce(E) -> U>(self, fun: F) -> Result<T, CommandQueueError<U>> {
        self.map_err(|err| err.map(fun))
    }

    fn into_ok_or_failed(self) -> Option<Result<T, E>> {
        match self {
            Ok(ok) => Some(Ok(ok)),
            Err(CommandQueueError::Cancelled) => None,
            Err(CommandQueueError::Failed(err)) => Some(Err(err)),
        }
    }
}

/// A message send to the dispatch task.
enum ForegroundMessage {
    SolveCondaEnvironment(SolveCondaEnvironmentTask),
    SolvePixiEnvironment(SolvePixiEnvironmentTask),
    SourceMetadata(SourceMetadataTask),
    GitCheckout(GitCheckoutTask),
}

/// A message that is send to the background task to start solving a particular
/// pixi environment.
struct SolvePixiEnvironmentTask {
    env: PixiEnvironmentSpec,
    context: Option<CommandQueueContext>,
    tx: oneshot::Sender<Result<Vec<PixiRecord>, SolvePixiEnvironmentError>>,
}

/// A message that is send to the background task to start solving a particular
/// conda environment.
struct SolveCondaEnvironmentTask {
    env: CondaEnvironmentSpec,
    context: Option<CommandQueueContext>,
    tx: oneshot::Sender<Result<Vec<RepoDataRecord>, SolveCondaEnvironmentError>>,
}

/// A message that is send to the background task to requesting the metadata for
/// a particular source spec.
struct SourceMetadataTask {
    spec: SourceMetadataSpec,
    context: Option<CommandQueueContext>,
    tx: oneshot::Sender<Result<SourceMetadata, SourceMetadataError>>,
}

impl Default for CommandQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandQueue {
    /// Constructs a new default constructed instance.
    pub fn new() -> Self {
        Self::builder().finish()
    }

    /// Constructs a new builder for the command_queue.
    pub fn builder() -> CommandQueueBuilder {
        CommandQueueBuilder::default()
    }

    /// Returns the gateway used to query conda repodata.
    pub fn gateway(&self) -> &Gateway {
        &self.data.gateway
    }

    /// Returns any build backend overrides
    pub fn build_backend_overrides(&self) -> &BackendOverride {
        &self.data.build_backend_overrides
    }

    /// Returns the cache directories used by the command queue.
    pub fn cache_dirs(&self) -> &CacheDirs {
        &self.data.cache_dirs
    }

    /// Returns the glob hash cache.
    pub fn glob_hash_cache(&self) -> &GlobHashCache {
        &self.data.glob_hash_cache
    }

    /// Returns the metadata of the source spec.
    pub async fn source_metadata(
        &self,
        spec: SourceMetadataSpec,
    ) -> Result<SourceMetadata, CommandQueueError<SourceMetadataError>> {
        let Some(sender) = self.channel.sender() else {
            // If this fails, it means the command_queue was dropped and the task is
            // immediately canceled.
            return Err(CommandQueueError::Cancelled);
        };

        let (tx, rx) = oneshot::channel();
        sender
            .send(ForegroundMessage::SourceMetadata(SourceMetadataTask {
                spec,
                context: self.context,
                tx,
            }))
            .map_err(|_| CommandQueueError::Cancelled)?;
        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(err)) => Err(CommandQueueError::Failed(err)),
            Err(_) => Err(CommandQueueError::Cancelled),
        }
    }

    /// Solves a particular pixi environment.
    pub async fn solve_pixi_environment(
        &self,
        env: PixiEnvironmentSpec,
    ) -> Result<Vec<PixiRecord>, CommandQueueError<SolvePixiEnvironmentError>> {
        let Some(sender) = self.channel.sender() else {
            // If this fails, it means the command_queue was dropped and the task is
            // immediately canceled.
            return Err(CommandQueueError::Cancelled);
        };

        let (tx, rx) = oneshot::channel();
        sender
            .send(ForegroundMessage::SolvePixiEnvironment(
                SolvePixiEnvironmentTask {
                    env,
                    context: self.context,
                    tx,
                },
            ))
            .map_err(|_| CommandQueueError::Cancelled)?;
        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(err)) => Err(CommandQueueError::Failed(err)),
            Err(_) => Err(CommandQueueError::Cancelled),
        }
    }

    /// Solves a particular conda environment.
    pub async fn solve_conda_environment(
        &self,
        env: CondaEnvironmentSpec,
    ) -> Result<Vec<RepoDataRecord>, CommandQueueError<SolveCondaEnvironmentError>> {
        let Some(sender) = self.channel.sender() else {
            // If this fails, it means the command_queue was dropped and the task is
            // immediately canceled.
            return Err(CommandQueueError::Cancelled);
        };

        let (tx, rx) = oneshot::channel();
        sender
            .send(ForegroundMessage::SolveCondaEnvironment(
                SolveCondaEnvironmentTask {
                    env,
                    context: self.context,
                    tx,
                },
            ))
            .map_err(|_| CommandQueueError::Cancelled)?;
        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(err)) => Err(CommandQueueError::Failed(err)),
            Err(_) => Err(CommandQueueError::Cancelled),
        }
    }

    /// Checks out a particular source based on a source spec.
    pub async fn pin_and_checkout(
        &self,
        source_spec: SourceSpec,
    ) -> Result<SourceCheckout, CommandQueueError<SourceCheckoutError>> {
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
}

impl CommandQueueData {
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
pub struct CommandQueueBuilder {
    gateway: Option<Gateway>,
    root_dir: Option<PathBuf>,
    reporter: Option<Box<dyn Reporter>>,
    git_resolver: Option<GitResolver>,
    client: Option<ClientWithMiddleware>,
    cache_dirs: Option<CacheDirs>,
    build_backend_overrides: BackendOverride,
}

impl CommandQueueBuilder {
    /// The cache directory to use
    pub fn with_cache_dirs(self, cache_dirs: CacheDirs) -> Self {
        Self {
            cache_dirs: Some(cache_dirs),
            ..self
        }
    }

    /// Sets the reporter used by the [`CommandQueue`] to report progress.
    pub fn with_reporter<F: Reporter + 'static>(self, reporter: F) -> Self {
        Self {
            reporter: Some(Box::new(reporter)),
            ..self
        }
    }

    /// Sets the reqwest client to use for network fetches.
    pub fn with_client(self, client: ClientWithMiddleware) -> Self {
        Self {
            client: Some(client),
            ..self
        }
    }

    /// Sets the git resolver used to fetch git repositories
    pub fn with_git_resolver(self, resolver: GitResolver) -> Self {
        Self {
            git_resolver: Some(resolver),
            ..self
        }
    }

    /// Sets the root directory which serves as the base directory when dealing
    /// with relative paths.
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

    /// Finish building the [`CommandQueue`] and return it.
    pub fn finish(self) -> CommandQueue {
        let root_dir = self
            .root_dir
            .or(std::env::current_dir().ok())
            .unwrap_or_default();
        let cache_dirs = self
            .cache_dirs
            .unwrap_or_else(|| CacheDirs::new(root_dir.join(".cache")));
        let client = self.client.unwrap_or_default();
        let gateway = self.gateway.unwrap_or_else(|| {
            Gateway::builder()
                .with_client(client.clone())
                .with_cache_dir(cache_dirs.root().clone())
                .finish()
        });

        let git_resolver = self.git_resolver.unwrap_or_default();

        let data = Arc::new(CommandQueueData {
            gateway,
            root_dir,
            git_resolver,
            cache_dirs,
            client,
            build_backend_overrides: self.build_backend_overrides,
            glob_hash_cache: GlobHashCache::default(),
        });

        let sender = CommandQueueProcessor::spawn(data.clone(), self.reporter);
        CommandQueue {
            channel: CommandQueueChannel::Strong(sender),
            context: None,
            data,
        }
    }
}
