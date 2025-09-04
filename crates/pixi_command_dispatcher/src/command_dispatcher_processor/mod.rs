//! This module defines the [`CommandDispatcherProcessor`]. This is a task
//! spawned on the background that coordinates certain computation requests.
//!
//! Because the task runs in a single thread ownership of its resources is much
//! simpler since we easily acquire mutable access to its fields.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use crate::{
    BuildBackendMetadata, BuildBackendMetadataError, BuildBackendMetadataSpec,
    CommandDispatcherErrorResultExt, InstallPixiEnvironmentResult, Reporter,
    SolveCondaEnvironmentSpec, SolvePixiEnvironmentError, SourceBuildCacheEntry,
    SourceBuildCacheStatusError, SourceBuildCacheStatusSpec, SourceBuildError, SourceBuildResult,
    SourceBuildSpec, SourceMetadata, SourceMetadataError, SourceMetadataSpec,
    backend_source_build::{BackendBuiltSource, BackendSourceBuildError, BackendSourceBuildSpec},
    command_dispatcher::{
        BackendSourceBuildId, BuildBackendMetadataId, CommandDispatcher, CommandDispatcherChannel,
        CommandDispatcherContext, CommandDispatcherData, CommandDispatcherError, ForegroundMessage,
        InstallPixiEnvironmentId, InstantiatedToolEnvId, SolveCondaEnvironmentId,
        SolvePixiEnvironmentId, SourceBuildCacheStatusId, SourceBuildId, SourceMetadataId,
    },
    executor::ExecutorFutures,
    install_pixi::InstallPixiEnvironmentError,
    instantiate_tool_env::{InstantiateToolEnvironmentError, InstantiateToolEnvironmentResult},
    reporter,
    solve_conda::SolveCondaEnvironmentError,
};
use futures::{StreamExt, future::LocalBoxFuture};
use itertools::Itertools;
use pixi_git::{GitError, resolver::RepositoryReference, source::Fetch};
use pixi_record::PixiRecord;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

mod backend_build_source;
mod build_backend_metadata;
mod git;
mod install_pixi;
mod instantiate_tool_env;
mod solve_conda;
mod solve_pixi;
mod source_build;
mod source_build_cache_status;
mod source_metadata;

/// Runs the command_dispatcher background task
pub(crate) struct CommandDispatcherProcessor {
    /// The receiver for messages from a [`CommandDispatcher]`.
    receiver: mpsc::UnboundedReceiver<ForegroundMessage>,

    /// Keeps track of the parent context for each task that is being processed.
    parent_contexts: HashMap<CommandDispatcherContext, CommandDispatcherContext>,

    /// A weak reference to the sender. This is used to allow constructing new
    /// [`Dispatchers`] without keeping the channel alive if there are no
    /// dispatchers alive. This is important because the command_dispatcher
    /// background task is only stopped once all senders (and thus
    /// dispatchers) have been dropped.
    sender: mpsc::WeakUnboundedSender<ForegroundMessage>,

    /// Data associated with dispatchers.
    inner: Arc<CommandDispatcherData>,

    /// Conda environments that are currently being solved.
    conda_solves: slotmap::SlotMap<SolveCondaEnvironmentId, PendingSolveCondaEnvironment>,

    /// A list of conda environments that are pending to be solved. These have
    /// not yet been queued for processing.
    pending_conda_solves: VecDeque<(
        SolveCondaEnvironmentId,
        SolveCondaEnvironmentSpec,
        CancellationToken,
    )>,

    /// Pixi environments that are currently being solved.
    solve_pixi_environments: slotmap::SlotMap<SolvePixiEnvironmentId, PendingPixiEnvironment>,

    /// Pixi environments that are currently being solved.
    install_pixi_environment:
        slotmap::SlotMap<InstallPixiEnvironmentId, PendingInstallPixiEnvironment>,

    /// A mapping of build backend metadata to the metadata id that
    build_backend_metadata: HashMap<
        BuildBackendMetadataId,
        PendingDeduplicatingTask<Arc<BuildBackendMetadata>, BuildBackendMetadataError>,
    >,
    build_backend_metadata_reporters:
        HashMap<BuildBackendMetadataId, reporter::BuildBackendMetadataId>,
    build_backend_metadata_ids: HashMap<BuildBackendMetadataSpec, BuildBackendMetadataId>,

    /// A mapping of source metadata to the metadata id that
    source_metadata: HashMap<
        SourceMetadataId,
        PendingDeduplicatingTask<Arc<SourceMetadata>, SourceMetadataError>,
    >,
    source_metadata_reporters: HashMap<SourceMetadataId, reporter::SourceMetadataId>,
    source_metadata_ids: HashMap<SourceMetadataSpec, SourceMetadataId>,

    /// A mapping of instantiated tool environments
    instantiated_tool_envs: HashMap<
        InstantiatedToolEnvId,
        PendingDeduplicatingTask<InstantiateToolEnvironmentResult, InstantiateToolEnvironmentError>,
    >,
    instantiated_tool_envs_reporters:
        HashMap<InstantiatedToolEnvId, reporter::InstantiateToolEnvId>,
    instantiated_tool_cache_keys: HashMap<String, InstantiatedToolEnvId>,

    /// Git checkouts in the process of being checked out, or already checked
    /// out.
    git_checkouts: HashMap<RepositoryReference, PendingGitCheckout>,

    /// Source builds that are currently being processed.
    source_build:
        HashMap<SourceBuildId, PendingDeduplicatingTask<SourceBuildResult, SourceBuildError>>,
    source_build_reporters: HashMap<SourceBuildId, reporter::SourceBuildId>,
    source_build_ids: HashMap<SourceBuildSpec, SourceBuildId>,

    /// Queries of source builds cache that are currently being processed.
    source_build_cache_status: HashMap<
        SourceBuildCacheStatusId,
        PendingDeduplicatingTask<Arc<SourceBuildCacheEntry>, SourceBuildCacheStatusError>,
    >,
    source_build_cache_status_ids: HashMap<SourceBuildCacheStatusSpec, SourceBuildCacheStatusId>,

    /// Backend source builds that are currently being processed.
    backend_source_builds: slotmap::SlotMap<BackendSourceBuildId, PendingBackendSourceBuild>,
    pending_backend_source_builds: VecDeque<(
        BackendSourceBuildId,
        BackendSourceBuildSpec,
        CancellationToken,
    )>,

    /// Keeps track of all pending futures. We poll them manually instead of
    /// spawning them so they can be `!Send` and because they are dropped when
    /// this instance is dropped.
    pending_futures: ExecutorFutures<LocalBoxFuture<'static, TaskResult>>,

    /// The reporter to use for reporting progress
    reporter: Option<Box<dyn Reporter>>,
}

/// A result of a task that was executed by the command_dispatcher background
/// task.
enum TaskResult {
    SolveCondaEnvironment(
        SolveCondaEnvironmentId,
        Result<Vec<PixiRecord>, CommandDispatcherError<SolveCondaEnvironmentError>>,
    ),
    SolvePixiEnvironment(
        SolvePixiEnvironmentId,
        Result<Vec<PixiRecord>, CommandDispatcherError<SolvePixiEnvironmentError>>,
    ),
    BuildBackendMetadata(
        BuildBackendMetadataId,
        Result<Arc<BuildBackendMetadata>, CommandDispatcherError<BuildBackendMetadataError>>,
    ),
    SourceMetadata(
        SourceMetadataId,
        Result<Arc<SourceMetadata>, CommandDispatcherError<SourceMetadataError>>,
    ),
    GitCheckedOut(
        RepositoryReference,
        Result<Fetch, CommandDispatcherError<GitError>>,
    ),
    InstallPixiEnvironment(
        InstallPixiEnvironmentId,
        Result<InstallPixiEnvironmentResult, CommandDispatcherError<InstallPixiEnvironmentError>>,
    ),
    InstantiateToolEnv(
        InstantiatedToolEnvId,
        Result<
            InstantiateToolEnvironmentResult,
            CommandDispatcherError<InstantiateToolEnvironmentError>,
        >,
    ),
    SourceBuild(
        SourceBuildId,
        Result<SourceBuildResult, CommandDispatcherError<SourceBuildError>>,
    ),
    QuerySourceBuildCache(
        SourceBuildCacheStatusId,
        Result<SourceBuildCacheEntry, CommandDispatcherError<SourceBuildCacheStatusError>>,
    ),
    BackendSourceBuild(
        BackendSourceBuildId,
        Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>>,
    ),
}

/// An either pending or already checked out git repository.
enum PendingGitCheckout {
    /// The checkout is still ongoing.
    Pending(
        Option<reporter::GitCheckoutId>,
        Vec<oneshot::Sender<Result<Fetch, GitError>>>,
    ),

    /// The repository was checked out and the result is available.
    CheckedOut(Fetch),

    /// A previous attempt failed
    Errored,
}

/// Information about a pending conda environment solve. This is used by the
/// background task to keep track of which command_dispatcher is awaiting the
/// result.
struct PendingSolveCondaEnvironment {
    tx: oneshot::Sender<Result<Vec<PixiRecord>, SolveCondaEnvironmentError>>,
    reporter_id: Option<reporter::CondaSolveId>,
}

struct PendingBackendSourceBuild {
    tx: oneshot::Sender<Result<BackendBuiltSource, BackendSourceBuildError>>,
    reporter_id: Option<reporter::BackendSourceBuildId>,
}

/// Information about a pending pixi environment solve. This is used by the
/// background task to keep track of which command_dispatcher is awaiting the
/// result.
struct PendingPixiEnvironment {
    tx: oneshot::Sender<Result<Vec<PixiRecord>, SolvePixiEnvironmentError>>,
    reporter_id: Option<reporter::PixiSolveId>,
}

/// Information about a pending pixi environment installation. This is used by
/// the background task to keep track of which command_dispatcher is awaiting
/// the result.
struct PendingInstallPixiEnvironment {
    tx: oneshot::Sender<Result<InstallPixiEnvironmentResult, InstallPixiEnvironmentError>>,
    reporter_id: Option<reporter::PixiInstallId>,
}

/// Describes information a pending task that is being deduplicated. Multiple
/// tasks can come in which are deduplicated, every task is returned the result
/// when available.
enum PendingDeduplicatingTask<T, E> {
    /// Task is currently executing, contains channels to notify when complete
    Pending(
        Vec<oneshot::Sender<Result<T, E>>>,
        Option<CommandDispatcherContext>,
    ),

    /// Task has completed successfully, result is cached
    Result(T, Option<CommandDispatcherContext>),

    /// Task has failed, future requests will also fail
    Errored,
}

impl<T: Clone, E> PendingDeduplicatingTask<T, E> {
    /// The result was received and all pending tasks can be notified.
    pub fn on_pending_result(&mut self, result: Result<T, CommandDispatcherError<E>>) {
        let Self::Pending(pending, context) = self else {
            unreachable!("cannot get a result for a task that is not pending");
        };

        let Some(result) = result.into_ok_or_failed() else {
            *self = Self::Errored;
            return;
        };

        match result {
            Ok(output) => {
                for tx in pending.drain(..) {
                    let _ = tx.send(Ok(output.clone()));
                }

                *self = Self::Result(output, *context);
            }
            Err(mut err) => {
                // Only send the error to the first channel, drop the rest, which cancels them.
                for tx in pending.drain(..) {
                    match tx.send(Err(err)) {
                        Ok(_) => return,
                        Err(Err(failed_to_send)) => err = failed_to_send,
                        Err(Ok(_)) => unreachable!(),
                    }
                }

                *self = Self::Errored;
            }
        }
    }
}

impl CommandDispatcherProcessor {
    /// Spawns a new background task that will handle the orchestration of all
    /// the dispatchers.
    ///
    /// The task is spawned on its own dedicated thread but uses the tokio
    /// runtime of the current thread to facilitate concurrency. Futures that
    /// are spawned on the background thread can still fully utilize tokio's
    /// runtime. Similarly, futures using tokios IO eventloop still work as
    /// expected.
    pub fn spawn(
        inner: Arc<CommandDispatcherData>,
        reporter: Option<Box<dyn Reporter>>,
    ) -> (
        mpsc::UnboundedSender<ForegroundMessage>,
        std::thread::JoinHandle<()>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let weak_tx = tx.downgrade();
        let rt = tokio::runtime::Handle::current();
        let join_handle = std::thread::spawn(move || {
            let task = Self {
                receiver: rx,
                parent_contexts: HashMap::new(),
                sender: weak_tx,
                conda_solves: slotmap::SlotMap::default(),
                pending_conda_solves: VecDeque::new(),
                solve_pixi_environments: slotmap::SlotMap::default(),
                install_pixi_environment: slotmap::SlotMap::default(),
                build_backend_metadata: HashMap::default(),
                build_backend_metadata_reporters: HashMap::default(),
                build_backend_metadata_ids: HashMap::default(),
                source_metadata: HashMap::default(),
                source_metadata_reporters: HashMap::default(),
                source_metadata_ids: HashMap::default(),
                instantiated_tool_envs: HashMap::default(),
                instantiated_tool_envs_reporters: HashMap::default(),
                instantiated_tool_cache_keys: HashMap::default(),
                git_checkouts: HashMap::default(),
                source_build: HashMap::default(),
                source_build_reporters: HashMap::default(),
                source_build_ids: HashMap::default(),
                source_build_cache_status: Default::default(),
                source_build_cache_status_ids: Default::default(),
                backend_source_builds: Default::default(),
                pending_backend_source_builds: Default::default(),
                pending_futures: ExecutorFutures::new(inner.executor),
                inner,
                reporter,
            };
            rt.block_on(task.run());
        });
        (tx, join_handle)
    }

    /// The main loop of the command_dispatcher background task. This function
    /// will run until all dispatchers have been dropped.
    async fn run(mut self) {
        tracing::trace!("Dispatch background task has started");
        if let Some(reporter) = self.reporter.as_mut() {
            reporter.on_start();
        }
        loop {
            tokio::select! {
                message = self.receiver.recv() => {
                    match message {
                        Some(message) => self.on_message(message),
                        None => {
                            // If all the senders are dropped, the receiver will be closed. When this
                            // happens, we can stop the command_dispatcher. All remaining tasks will be dropped
                            // as `self.pending_futures` is dropped.
                            break;
                        }
                    }
                }
                Some(result) = self.pending_futures.next() => {
                    self.on_result(result);
                }
            }
        }
        if let Some(reporter) = self.reporter.as_mut() {
            reporter.on_finished();
        }
        tracing::trace!("Dispatch background task has finished");
    }

    /// Called when a message was received from either the command_dispatcher or
    /// another task.
    fn on_message(&mut self, message: ForegroundMessage) {
        match message {
            ForegroundMessage::SolveCondaEnvironment(task) => self.on_solve_conda_environment(task),
            ForegroundMessage::SolvePixiEnvironment(task) => self.on_solve_pixi_environment(task),
            ForegroundMessage::InstallPixiEnvironment(task) => {
                self.on_install_pixi_environment(task)
            }
            ForegroundMessage::InstantiateToolEnvironment(task) => {
                self.on_instantiate_tool_environment(task)
            }
            ForegroundMessage::BuildBackendMetadata(task) => self.on_build_backend_metadata(task),
            ForegroundMessage::GitCheckout(task) => self.on_checkout_git(task),
            ForegroundMessage::SourceBuild(task) => self.on_source_build(task),
            ForegroundMessage::QuerySourceBuildCache(task) => {
                self.on_source_build_cache_status(task)
            }
            ForegroundMessage::ClearReporter(sender) => self.clear_reporter(sender),
            ForegroundMessage::SourceMetadata(task) => self.on_source_metadata(task),
            ForegroundMessage::BackendSourceBuild(task) => self.on_backend_source_build(task),
        }
    }

    /// Called when the result of a task was received.
    fn on_result(&mut self, result: TaskResult) {
        match result {
            TaskResult::SolveCondaEnvironment(id, result) => {
                self.on_solve_conda_environment_result(id, result)
            }
            TaskResult::SolvePixiEnvironment(id, result) => {
                self.on_solve_pixi_environment_result(id, result)
            }
            TaskResult::InstallPixiEnvironment(id, result) => {
                self.on_install_pixi_environment_result(id, result)
            }
            TaskResult::BuildBackendMetadata(id, result) => {
                self.on_build_backend_metadata_result(id, result)
            }
            TaskResult::GitCheckedOut(url, result) => self.on_git_checked_out(url, result),
            TaskResult::InstantiateToolEnv(id, result) => {
                self.on_instantiate_tool_environment_result(id, result)
            }
            TaskResult::SourceBuild(id, result) => self.on_source_build_result(id, result),
            TaskResult::SourceMetadata(id, result) => self.on_source_metadata_result(id, result),
            TaskResult::BackendSourceBuild(id, result) => {
                self.on_backend_source_build_result(id, result)
            }
            TaskResult::QuerySourceBuildCache(id, result) => {
                self.on_source_build_cache_status_result(id, result)
            }
        }
    }

    /// Constructs a new [`CommandDispatcher`] that can be used for tasks
    /// constructed by the command dispatcher process itself.
    fn create_task_command_dispatcher(
        &self,
        context: CommandDispatcherContext,
    ) -> CommandDispatcher {
        CommandDispatcher {
            channel: Some(CommandDispatcherChannel::Weak(self.sender.clone())),
            context: Some(context),
            data: self.inner.clone(),
            processor_handle: None,
        }
    }

    fn reporter_context(
        &self,
        context: CommandDispatcherContext,
    ) -> Option<reporter::ReporterContext> {
        let mut parent_context = Some(context);
        while let Some(context) = parent_context.take() {
            parent_context = match context {
                CommandDispatcherContext::SolveCondaEnvironment(id) => {
                    return self.conda_solves[id]
                        .reporter_id
                        .map(reporter::ReporterContext::SolveConda);
                }
                CommandDispatcherContext::SolvePixiEnvironment(id) => {
                    return self.solve_pixi_environments[id]
                        .reporter_id
                        .map(reporter::ReporterContext::SolvePixi);
                }
                CommandDispatcherContext::BuildBackendMetadata(id) => {
                    if let Some(context) = self
                        .build_backend_metadata_reporters
                        .get(&id)
                        .copied()
                        .map(reporter::ReporterContext::BuildBackendMetadata)
                    {
                        return Some(context);
                    }

                    self.build_backend_metadata
                        .get(&id)
                        .and_then(|pending| match pending {
                            PendingDeduplicatingTask::Pending(_, context) => Some(*context),
                            PendingDeduplicatingTask::Result(_, context) => Some(*context),
                            PendingDeduplicatingTask::Errored => None,
                        })?
                }
                CommandDispatcherContext::SourceMetadata(id) => {
                    if let Some(context) = self
                        .source_metadata_reporters
                        .get(&id)
                        .copied()
                        .map(reporter::ReporterContext::SourceMetadata)
                    {
                        return Some(context);
                    }

                    self.source_metadata
                        .get(&id)
                        .and_then(|pending| match pending {
                            PendingDeduplicatingTask::Pending(_, context) => Some(*context),
                            PendingDeduplicatingTask::Result(_, context) => Some(*context),
                            PendingDeduplicatingTask::Errored => None,
                        })?
                }
                CommandDispatcherContext::InstallPixiEnvironment(id) => {
                    return self.install_pixi_environment[id]
                        .reporter_id
                        .map(reporter::ReporterContext::InstallPixi);
                }
                CommandDispatcherContext::InstantiateToolEnv(id) => {
                    if let Some(context) = self
                        .instantiated_tool_envs_reporters
                        .get(&id)
                        .copied()
                        .map(reporter::ReporterContext::InstantiateToolEnv)
                    {
                        return Some(context);
                    }

                    self.instantiated_tool_envs
                        .get(&id)
                        .and_then(|pending| match pending {
                            PendingDeduplicatingTask::Pending(_, context) => Some(*context),
                            PendingDeduplicatingTask::Result(_, context) => Some(*context),
                            PendingDeduplicatingTask::Errored => None,
                        })?
                }
                CommandDispatcherContext::SourceBuild(id) => {
                    if let Some(context) = self
                        .source_build_reporters
                        .get(&id)
                        .copied()
                        .map(reporter::ReporterContext::SourceBuild)
                    {
                        return Some(context);
                    }

                    self.source_build
                        .get(&id)
                        .and_then(|pending| match pending {
                            PendingDeduplicatingTask::Pending(_, context) => Some(*context),
                            PendingDeduplicatingTask::Result(_, context) => Some(*context),
                            PendingDeduplicatingTask::Errored => None,
                        })?
                }
                CommandDispatcherContext::BackendSourceBuild(id) => {
                    return self.backend_source_builds[id]
                        .reporter_id
                        .map(reporter::ReporterContext::BackendSourceBuild);
                }
                CommandDispatcherContext::QuerySourceBuildCache(_id) => {
                    return None;
                }
            };
        }

        None
    }

    /// Called to clear the reporter.
    fn clear_reporter(&mut self, sender: oneshot::Sender<()>) {
        if let Some(reporter) = self.reporter.as_mut() {
            reporter.on_clear()
        }
        let _ = sender.send(());
    }

    /// Returns true if by following the parent chain of the `parent` context we
    /// stumble on `id`.
    pub fn contains_cycle<T: TryFrom<CommandDispatcherContext> + PartialEq>(
        &self,
        id: T,
        parent: Option<CommandDispatcherContext>,
    ) -> bool {
        std::iter::successors(parent, |ctx| self.parent_contexts.get(ctx).cloned())
            .filter_map(|context| T::try_from(context).ok())
            .contains(&id)
    }
}
