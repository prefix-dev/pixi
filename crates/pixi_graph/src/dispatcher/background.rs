use std::sync::Arc;

use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::FuturesUnordered};
use pixi_record::PixiRecord;
use tokio::sync::{mpsc, oneshot};

use super::{
    DispatchChannel, DispatchInner, DispatchMessage, Dispatcher, DispatcherContext,
    SolveCondaEnvironmentId, SolveCondaEnvironmentTask,
};
use crate::{DispatchError, SolveCondaEnvironmentError};

/// Runs the dispatcher background task
pub(super) struct DispatcherBackgroundTask {
    /// The receiver for messages from a [`Dispatcher]`.
    receiver: mpsc::UnboundedReceiver<DispatchMessage>,

    /// A weak reference to the sender. This is used to allow constructing new
    /// [`Dispatchers`] without keeping the channel alive if there are no
    /// dispatchers alive. This is important because the dispatcher background
    /// task is only stopped once all senders (and thus dispatchers) have been
    /// dropped.
    sender: mpsc::WeakUnboundedSender<DispatchMessage>,

    /// Data associated with dispatchers.
    inner: Arc<DispatchInner>,

    /// Conda environments that are currently being solved.
    conda_environments: slotmap::SlotMap<SolveCondaEnvironmentId, PendingCondaEnvironment>,

    /// Keeps track of all pending futures. We poll them manually instead of
    /// spawning them so they can be `!Send` and because they are dropped when
    /// this instance is dropped.
    pending_futures: FuturesUnordered<LocalBoxFuture<'static, DispatchResult>>,
}

/// A result of a task that was executed by the dispatcher background task.
enum DispatchResult {
    SolveCondaEnvironment(
        SolveCondaEnvironmentId,
        Result<Vec<PixiRecord>, DispatchError<SolveCondaEnvironmentError>>,
    ),
}

/// Information about a pending conda environment solve. This is used by the
/// background task to keep track of which dispatcher is awaiting the result.
struct PendingCondaEnvironment {
    tx: oneshot::Sender<Result<Vec<PixiRecord>, SolveCondaEnvironmentError>>,
}

impl DispatcherBackgroundTask {
    /// Spawns a new background task that will handle the orchestration of all
    /// the dispatchers.
    pub fn spawn(inner: Arc<DispatchInner>) -> mpsc::UnboundedSender<DispatchMessage> {
        let (tx, rx) = mpsc::unbounded_channel();
        let weak_tx = tx.downgrade();
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            let task = Self {
                receiver: rx,
                sender: weak_tx,
                conda_environments: slotmap::SlotMap::default(),
                pending_futures: FuturesUnordered::new(),
                inner,
            };
            rt.block_on(task.run());
        });
        tx
    }

    /// The main loop of the dispatcher background task. This function will run
    /// until all dispatchers have been dropped.
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
                    // happens, we can stop the dispatcher. All remaining tasks will be dropped
                    // as `self.pending_futures` is dropped.
                    break
                },
            }
        }
        tracing::debug!("Dispatch background task has finished");
    }

    /// Called when the result of a task was received.
    fn on_result(&mut self, result: DispatchResult) {
        match result {
            DispatchResult::SolveCondaEnvironment(id, result) => {
                self.on_solve_environment_result(id, result)
            }
        }
    }

    /// Called when a message was received from either the dispatcher or another
    /// task.
    fn on_message(&mut self, message: DispatchMessage) {
        match message {
            DispatchMessage::SolveCondaEnvironment(task) => self.on_solve_environment(task),
        }
    }

    /// Called when a [`DispatchMessage::SolveCondaEnvironment`] task was
    /// received.
    fn on_solve_environment(&mut self, task: SolveCondaEnvironmentTask) {
        let pending_env_id = self
            .conda_environments
            .insert(PendingCondaEnvironment { tx: task.tx });

        // Construct a dispatcher for this task. This is used to track the tasks
        // executed as part of it.
        let dispatcher = Dispatcher {
            channel: DispatchChannel::Weak(self.sender.clone()),
            context: Some(DispatcherContext::SolveCondaEnvironment(pending_env_id)),
            inner: self.inner.clone(),
        };

        // Add the task to the list of pending futures.
        self.pending_futures.push(
            task.env
                .solve(dispatcher)
                .map(move |result| DispatchResult::SolveCondaEnvironment(pending_env_id, result))
                .boxed_local(),
        );
    }

    /// Called when a [`DispatchResult::SolveCondaEnvironment`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`Dispatcher`] that issues it.
    fn on_solve_environment_result(
        &mut self,
        id: SolveCondaEnvironmentId,
        result: Result<Vec<PixiRecord>, DispatchError<SolveCondaEnvironmentError>>,
    ) {
        let env = self
            .conda_environments
            .remove(id)
            .expect("got a result for a conda environment that was not pending");

        let result = match result {
            Err(DispatchError::Cancelled) => {
                // If the job was canceled, we can just drop the sending end
                // which will also cause a cancel on the receiving end.
                return;
            }
            Err(DispatchError::Failed(err)) => Err(err),
            Ok(result) => Ok(result),
        };

        // We can silently ignore the result if the task was cancelled.
        let _ = env.tx.send(result);
    }
}
