//! This module defines the [`CommandQueueProcessor`]. This is a task
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
use tokio::sync::{mpsc, oneshot};

use crate::command_queue::CommandQueueError;
use crate::{
    Reporter, SolveCondaEnvironmentSpec, SolvePixiEnvironmentError, SourceMetadataSpec,
    command_queue::{
        CommandQueue, CommandQueueChannel, CommandQueueContext, CommandQueueData,
        ForegroundMessage, SolveCondaEnvironmentId, SolvePixiEnvironmentId, SourceMetadataId,
    },
    executor::{Executor, ExecutorFutures},
    reporter,
    source_metadata::{SourceMetadata, SourceMetadataError},
};

mod conda;
mod git;
mod pixi;
mod source_metadata;

/// Runs the command_queue background task
pub(crate) struct CommandQueueProcessor {
    /// The receiver for messages from a [`CommandQueue]`.
    receiver: mpsc::UnboundedReceiver<ForegroundMessage>,

    /// A weak reference to the sender. This is used to allow constructing new
    /// [`Dispatchers`] without keeping the channel alive if there are no
    /// dispatchers alive. This is important because the command_queue
    /// background task is only stopped once all senders (and thus
    /// dispatchers) have been dropped.
    sender: mpsc::WeakUnboundedSender<ForegroundMessage>,

    /// Data associated with dispatchers.
    inner: Arc<CommandQueueData>,

    /// Conda environments that are currently being solved.
    conda_solves: slotmap::SlotMap<SolveCondaEnvironmentId, PendingSolveCondaEnvironment>,

    /// A list of conda environments that are pending to be solved. These have
    /// not yet been queued for processing.
    pending_conda_solves: VecDeque<(SolveCondaEnvironmentId, SolveCondaEnvironmentSpec)>,

    /// Pixi environments that are currently being solved.
    pixi_environments: slotmap::SlotMap<SolvePixiEnvironmentId, PendingPixiEnvironment>,

    /// A mapping of source metadata to the metadata id that
    source_metadata: HashMap<SourceMetadataId, PendingSourceMetadata>,
    source_metadata_ids: HashMap<SourceMetadataSpec, SourceMetadataId>,

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

/// A result of a task that was executed by the command_queue background task.
enum TaskResult {
    SolveCondaEnvironment(
        SolveCondaEnvironmentId,
        Result<Vec<PixiRecord>, CommandQueueError<rattler_solve::SolveError>>,
    ),
    SolvePixiEnvironment(
        SolvePixiEnvironmentId,
        Result<Vec<PixiRecord>, CommandQueueError<SolvePixiEnvironmentError>>,
    ),
    SourceMetadata(
        SourceMetadataId,
        Result<Arc<SourceMetadata>, CommandQueueError<SourceMetadataError>>,
    ),
    GitCheckedOut(GitUrl, Result<Fetch, GitError>),
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
/// background task to keep track of which command_queue is awaiting the result.
struct PendingSolveCondaEnvironment {
    tx: oneshot::Sender<Result<Vec<PixiRecord>, rattler_solve::SolveError>>,
    reporter_id: Option<reporter::CondaSolveId>,
}

/// Information about a pending pixi environment solve. This is used by the
/// background task to keep track of which command_queue is awaiting the result.
struct PendingPixiEnvironment {
    tx: oneshot::Sender<Result<Vec<PixiRecord>, SolvePixiEnvironmentError>>,
    reporter_id: Option<reporter::PixiSolveId>,
}

/// Information about a request for metadata of a particular source spec.
enum PendingSourceMetadata {
    Pending(Vec<oneshot::Sender<Result<Arc<SourceMetadata>, SourceMetadataError>>>),
    Result(Arc<SourceMetadata>),
    Errored,
}

impl CommandQueueProcessor {
    /// Spawns a new background task that will handle the orchestration of all
    /// the dispatchers.
    ///
    /// The task is spawned on its own dedicated thread but uses the tokio
    /// runtime of the current thread to facilitate concurrency. Futures that
    /// are spawned on the background thread can still fully utilize tokio's
    /// runtime. Similarly, futures using tokios IO eventloop still work as
    /// expected.
    pub fn spawn(
        inner: Arc<CommandQueueData>,
        reporter: Option<Box<dyn Reporter>>,
        executor: Executor,
    ) -> mpsc::UnboundedSender<ForegroundMessage> {
        let (tx, rx) = mpsc::unbounded_channel();
        let weak_tx = tx.downgrade();
        let rt = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            let task = Self {
                receiver: rx,
                sender: weak_tx,
                conda_solves: slotmap::SlotMap::default(),
                pending_conda_solves: VecDeque::new(),
                pixi_environments: slotmap::SlotMap::default(),
                source_metadata: HashMap::default(),
                source_metadata_ids: HashMap::default(),
                git_checkouts: HashMap::default(),
                pending_futures: ExecutorFutures::new(executor),
                inner,
                reporter,
            };
            rt.block_on(task.run());
        });
        tx
    }

    /// The main loop of the command_queue background task. This function will
    /// run until all dispatchers have been dropped.
    async fn run(mut self) {
        tracing::debug!("Dispatch background task has started");
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
                    // happens, we can stop the command_queue. All remaining tasks will be dropped
                    // as `self.pending_futures` is dropped.
                    break
                },
            }
        }
        tracing::debug!("Dispatch background task has finished");
    }

    /// Called when a message was received from either the command_queue or
    /// another task.
    fn on_message(&mut self, message: ForegroundMessage) {
        match message {
            ForegroundMessage::SolveCondaEnvironment(task) => self.on_solve_conda_environment(task),
            ForegroundMessage::SolvePixiEnvironment(task) => self.on_solve_pixi_environment(task),
            ForegroundMessage::SourceMetadata(task) => self.on_source_metadata(task),
            ForegroundMessage::GitCheckout(task) => self.on_checkout_git(task),
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
            TaskResult::SourceMetadata(id, result) => self.on_source_metadata_result(id, result),
            TaskResult::GitCheckedOut(url, result) => self.on_git_checked_out(url, result),
        }
    }

    /// Constructs a new [`CommandQueue`] that can be used for tasks constructed
    /// by the command_queue_processor itself.
    fn create_task_command_queue(&self, context: CommandQueueContext) -> CommandQueue {
        CommandQueue {
            channel: CommandQueueChannel::Weak(self.sender.clone()),
            context: Some(context),
            data: self.inner.clone(),
        }
    }
}
