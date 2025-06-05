//! This module defines the [`CommandDispatcherProcessor`]. This is a task
//! spawned on the background that coordinates certain computation requests.
//!
//! Because the task runs in a single thread ownership of its resources is much
//! simpler since we easily acquire mutable access to its fields.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use futures::{StreamExt, future::LocalBoxFuture};
use pixi_git::{GitError, GitUrl, source::Fetch};
use pixi_record::PixiRecord;
use rattler_conda_types::prefix::Prefix;
use tokio::sync::{mpsc, oneshot};

use crate::{
    CommandDispatcherErrorResultExt, Reporter, SolveCondaEnvironmentSpec,
    SolvePixiEnvironmentError, SourceMetadataSpec,
    command_dispatcher::{
        CommandDispatcher, CommandDispatcherChannel, CommandDispatcherContext,
        CommandDispatcherData, CommandDispatcherError, ForegroundMessage, InstallPixiEnvironmentId,
        InstantiatedToolEnvId, SolveCondaEnvironmentId, SolvePixiEnvironmentId, SourceMetadataId,
        TaskSpec,
    },
    executor::{Executor, ExecutorFutures},
    install_pixi::InstallPixiEnvironmentError,
    instantiate_tool_env::{InstantiateToolEnvironmentError, InstantiateToolEnvironmentSpec},
    reporter,
    source_metadata::{SourceMetadata, SourceMetadataError},
};

mod git;
mod install_pixi;
mod instantiate_tool_env;
mod solve_conda;
mod solve_pixi;
mod source_metadata;

/// Runs the command_dispatcher background task
pub(crate) struct CommandDispatcherProcessor {
    /// The receiver for messages from a [`CommandDispatcher]`.
    receiver: mpsc::UnboundedReceiver<ForegroundMessage>,

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
    pending_conda_solves: VecDeque<(SolveCondaEnvironmentId, SolveCondaEnvironmentSpec)>,

    /// Pixi environments that are currently being solved.
    solve_pixi_environments: slotmap::SlotMap<SolvePixiEnvironmentId, PendingPixiEnvironment>,

    /// Pixi environments that are currently being solved.
    install_pixi_environment:
        slotmap::SlotMap<InstallPixiEnvironmentId, PendingInstallPixiEnvironment>,

    /// A mapping of source metadata to the metadata id that
    source_metadata: HashMap<SourceMetadataId, PendingSourceMetadata>,
    source_metadata_reporters: HashMap<SourceMetadataId, reporter::SourceMetadataId>,
    source_metadata_ids: HashMap<SourceMetadataSpec, SourceMetadataId>,

    /// A mapping of instantiated tool environments
    instantiated_tool_envs:
        HashMap<InstantiatedToolEnvId, PendingDeduplicatingTask<InstantiateToolEnvironmentSpec>>,
    instantiated_tool_envs_reporters:
        HashMap<InstantiatedToolEnvId, reporter::InstantiateToolEnvId>,
    instantiated_tool_cache_keys: HashMap<String, InstantiatedToolEnvId>,

    /// Git checkouts in the process of being checked out, or already checked
    /// out.
    git_checkouts: HashMap<GitUrl, PendingGitCheckout>,

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
        Result<Vec<PixiRecord>, CommandDispatcherError<rattler_solve::SolveError>>,
    ),
    SolvePixiEnvironment(
        SolvePixiEnvironmentId,
        Result<Vec<PixiRecord>, CommandDispatcherError<SolvePixiEnvironmentError>>,
    ),
    SourceMetadata(
        SourceMetadataId,
        Result<Arc<SourceMetadata>, CommandDispatcherError<SourceMetadataError>>,
    ),
    GitCheckedOut(GitUrl, Result<Fetch, GitError>),
    InstallPixiEnvironment(
        InstallPixiEnvironmentId,
        Result<(), CommandDispatcherError<InstallPixiEnvironmentError>>,
    ),
    InstantiateToolEnv(
        InstantiatedToolEnvId,
        Result<Prefix, CommandDispatcherError<InstantiateToolEnvironmentError>>,
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
    tx: oneshot::Sender<Result<Vec<PixiRecord>, rattler_solve::SolveError>>,
    reporter_id: Option<reporter::CondaSolveId>,
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
    tx: oneshot::Sender<Result<(), InstallPixiEnvironmentError>>,
    reporter_id: Option<reporter::PixiInstallId>,
}

/// Describes information a pending task that is being deduplicated. Multiple
/// tasks can come in which are deduplicated, every task is returned the result
/// when available.
enum PendingDeduplicatingTask<T: TaskSpec> {
    /// Task is currently executing, contains channels to notify when complete
    Pending(
        Vec<oneshot::Sender<Result<T::Output, T::Error>>>,
        Option<CommandDispatcherContext>,
    ),

    /// Task has completed successfully, result is cached
    Result(T::Output, Option<CommandDispatcherContext>),

    /// Task has failed, future requests will also fail
    Errored,
}

impl<T: TaskSpec> PendingDeduplicatingTask<T>
where
    T::Output: Clone,
{
    /// The result was received and all pending tasks can be notified.
    pub fn on_pending_result(
        &mut self,
        result: Result<T::Output, CommandDispatcherError<T::Error>>,
    ) {
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

/// Information about a request for metadata of a particular source spec.
enum PendingSourceMetadata {
    Pending(
        Vec<oneshot::Sender<Result<Arc<SourceMetadata>, SourceMetadataError>>>,
        Option<CommandDispatcherContext>,
    ),
    Result(Arc<SourceMetadata>, Option<CommandDispatcherContext>),
    Errored,
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
        executor: Executor,
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
                sender: weak_tx,
                conda_solves: slotmap::SlotMap::default(),
                pending_conda_solves: VecDeque::new(),
                solve_pixi_environments: slotmap::SlotMap::default(),
                install_pixi_environment: slotmap::SlotMap::default(),
                source_metadata: HashMap::default(),
                source_metadata_reporters: HashMap::default(),
                source_metadata_ids: HashMap::default(),
                instantiated_tool_envs: HashMap::default(),
                instantiated_tool_envs_reporters: HashMap::default(),
                instantiated_tool_cache_keys: HashMap::default(),
                git_checkouts: HashMap::default(),
                pending_futures: ExecutorFutures::new(executor),
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
                Some(message) = self.receiver.recv() => {
                    self.on_message(message);
                }
                Some(result) = self.pending_futures.next() => {
                    self.on_result(result);
                }
                else => {
                    // If all the senders are dropped, the receiver will be closed. When this
                    // happens, we can stop the command_dispatcher. All remaining tasks will be dropped
                    // as `self.pending_futures` is dropped.
                    break
                },
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
            ForegroundMessage::SourceMetadata(task) => self.on_source_metadata(task),
            ForegroundMessage::GitCheckout(task) => self.on_checkout_git(task),
            ForegroundMessage::ClearReporter(sender) => self.clear_reporter(sender),
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
            TaskResult::SourceMetadata(id, result) => self.on_source_metadata_result(id, result),
            TaskResult::GitCheckedOut(url, result) => self.on_git_checked_out(url, result),
            TaskResult::InstantiateToolEnv(id, result) => {
                self.on_instantiate_tool_environment_result(id, result)
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
                            PendingSourceMetadata::Pending(_, context) => Some(*context),
                            PendingSourceMetadata::Result(_, context) => Some(*context),
                            PendingSourceMetadata::Errored => None,
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
}
