use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Arc,
};

use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::FuturesUnordered};
use pixi_git::{GitError, GitUrl, resolver::RepositoryReference, source::Fetch};
use pixi_record::PixiRecord;
use rattler_conda_types::RepoDataRecord;
use tokio::sync::{mpsc, oneshot};

use super::{
    CommandQueue, CommandQueueChannel, CommandQueueContext, CommandQueueData,
    CommandQueueErrorResultExt, ForegroundMessage, GitCheckoutTask, SolveCondaEnvironmentId,
    SolveCondaEnvironmentTask, SolvePixiEnvironmentId, SolvePixiEnvironmentTask,
};
use crate::{
    CommandQueueError, Reporter, SolveCondaEnvironmentError, SolvePixiEnvironmentError, reporter,
};

/// Runs the command_queue background task
pub(super) struct CommandQueueProcessor {
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
    conda_environments: slotmap::SlotMap<SolveCondaEnvironmentId, PendingCondaEnvironment>,

    /// Pixi environments that are currently being solved.
    pixi_environments: slotmap::SlotMap<SolvePixiEnvironmentId, PendingPixiEnvironment>,

    /// Git checkouts in the process of being checked out, or already checked
    /// out.
    git_checkouts: HashMap<GitUrl, PendingGitCheckout>,

    /// Keeps track of all pending futures. We poll them manually instead of
    /// spawning them so they can be `!Send` and because they are dropped when
    /// this instance is dropped.
    pending_futures: FuturesUnordered<LocalBoxFuture<'static, TaskResult>>,

    /// The reporter to use for reporting progress
    reporter: Option<Box<dyn Reporter>>,
}

/// A result of a task that was executed by the command_queue background task.
enum TaskResult {
    SolveCondaEnvironment(
        SolveCondaEnvironmentId,
        Result<Vec<RepoDataRecord>, CommandQueueError<SolveCondaEnvironmentError>>,
    ),
    SolvePixiEnvironment(
        SolvePixiEnvironmentId,
        Result<Vec<PixiRecord>, CommandQueueError<SolvePixiEnvironmentError>>,
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
struct PendingCondaEnvironment {
    tx: oneshot::Sender<Result<Vec<RepoDataRecord>, SolveCondaEnvironmentError>>,
    reporter_id: Option<reporter::SolveId>,
}

/// Information about a pending pixi environment solve. This is used by the
/// background task to keep track of which command_queue is awaiting the result.
struct PendingPixiEnvironment {
    tx: oneshot::Sender<Result<Vec<PixiRecord>, SolvePixiEnvironmentError>>,
}

impl CommandQueueProcessor {
    /// Spawns a new background task that will handle the orchestration of all
    /// the dispatchers.
    pub fn spawn(
        inner: Arc<CommandQueueData>,
        reporter: Option<Box<dyn Reporter>>,
    ) -> mpsc::UnboundedSender<ForegroundMessage> {
        let (tx, rx) = mpsc::unbounded_channel();
        let weak_tx = tx.downgrade();
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            let task = Self {
                receiver: rx,
                sender: weak_tx,
                conda_environments: slotmap::SlotMap::default(),
                pixi_environments: slotmap::SlotMap::default(),
                git_checkouts: HashMap::default(),
                pending_futures: FuturesUnordered::new(),
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

    /// Called when the result of a task was received.
    fn on_result(&mut self, result: TaskResult) {
        match result {
            TaskResult::SolveCondaEnvironment(id, result) => {
                self.on_solve_conda_environment_result(id, result)
            }
            TaskResult::SolvePixiEnvironment(id, result) => {
                self.on_solve_pixi_environment_result(id, result)
            }
            TaskResult::GitCheckedOut(url, result) => self.on_git_checked_out(url, result),
        }
    }

    /// Called when a message was received from either the command_queue or
    /// another task.
    fn on_message(&mut self, message: ForegroundMessage) {
        match message {
            ForegroundMessage::SolveCondaEnvironment(task) => self.on_solve_conda_environment(task),
            ForegroundMessage::SolvePixiEnvironment(task) => self.on_solve_pixi_environment(task),
            ForegroundMessage::GitCheckout(task) => self.on_checkout_git(task),
        }
    }

    /// Constructs a new [`CommandQueue`] that can be used for tasks constructed
    /// by the processor itself.
    fn create_task_command_queue(&self, context: CommandQueueContext) -> CommandQueue {
        CommandQueue {
            channel: CommandQueueChannel::Weak(self.sender.clone()),
            context: Some(context),
            data: self.inner.clone(),
        }
    }

    /// Called when a [`ForegroundMessage::SolveCondaEnvironment`] task was
    /// received.
    fn on_solve_conda_environment(&mut self, task: SolveCondaEnvironmentTask) {
        // Notify the reporter that a new solve has been queued.
        let reporter_id = self
            .reporter
            .as_mut()
            .map(|reporter| reporter.on_solve_queued(&task.env));

        // Store information about the pending environment.
        let pending_env_id = self.conda_environments.insert(PendingCondaEnvironment {
            tx: task.tx,
            reporter_id,
        });

        // Notify the reporter that the solve has started.
        if let Some((reporter, id)) = self.reporter.as_mut().zip(reporter_id) {
            reporter.on_solve_start(id)
        }

        // Add the task to the list of pending futures.
        let dispatcher = self
            .create_task_command_queue(CommandQueueContext::SolveCondaEnvironment(pending_env_id));
        self.pending_futures.push(
            task.env
                .solve(dispatcher)
                .map(move |result| TaskResult::SolveCondaEnvironment(pending_env_id, result))
                .boxed_local(),
        );
    }

    /// Called when a [`TaskResult::SolveCondaEnvironment`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`CommandQueue`] that issues it.
    fn on_solve_conda_environment_result(
        &mut self,
        id: SolveCondaEnvironmentId,
        result: Result<Vec<RepoDataRecord>, CommandQueueError<SolveCondaEnvironmentError>>,
    ) {
        let env = self
            .conda_environments
            .remove(id)
            .expect("got a result for a conda environment that was not pending");

        // Notify the reporter that the solve finished.
        if let Some((reporter, id)) = self.reporter.as_mut().zip(env.reporter_id) {
            reporter.on_solve_start(id)
        }

        let Some(result) = result.into_ok_or_failed() else {
            // If the job was canceled, we can just drop the sending end
            // which will also cause a cancel on the receiving end.
            return;
        };

        // We can silently ignore the result if the task was cancelled.
        let _ = env.tx.send(result);
    }

    /// Called when a [`ForegroundMessage::SolvePixiEnvironmentTask`] task was
    /// received.
    fn on_solve_pixi_environment(&mut self, task: SolvePixiEnvironmentTask) {
        // Store information about the pending environment.
        let pending_env_id = self
            .pixi_environments
            .insert(PendingPixiEnvironment { tx: task.tx });

        // Add the task to the list of pending futures.
        let dispatcher = self
            .create_task_command_queue(CommandQueueContext::SolvePixiEnvironment(pending_env_id));
        self.pending_futures.push(
            task.env
                .solve(dispatcher)
                .map(move |result| TaskResult::SolvePixiEnvironment(pending_env_id, result))
                .boxed_local(),
        );
    }

    /// Called when a [`TaskResult::SolvePixiEnvironment`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`CommandQueue`] that issues it.
    fn on_solve_pixi_environment_result(
        &mut self,
        id: SolvePixiEnvironmentId,
        result: Result<Vec<PixiRecord>, CommandQueueError<SolvePixiEnvironmentError>>,
    ) {
        let env = self
            .pixi_environments
            .remove(id)
            .expect("got a result for a conda environment that was not pending");

        let Some(result) = result.into_ok_or_failed() else {
            // If the job was canceled, we can just drop the sending end
            // which will also cause a cancel on the receiving end.
            return;
        };

        // We can silently ignore the result if the task was cancelled.
        let _ = env.tx.send(result);
    }

    /// Called when a [`ForegroundMessage::GitCheckout`] task was received.
    fn on_checkout_git(&mut self, task: GitCheckoutTask) {
        match self.git_checkouts.entry(task.url.clone()) {
            Entry::Occupied(mut existing_checkout) => match existing_checkout.get_mut() {
                PendingGitCheckout::Pending(_, pending) => pending.push(task.tx),
                PendingGitCheckout::CheckedOut(fetch) => {
                    let _ = task.tx.send(Ok(fetch.clone()));
                }
                PendingGitCheckout::Errored => {
                    // Drop the sender, this will cause a cancellation on the other side.
                    drop(task.tx);
                }
            },
            Entry::Vacant(entry) => {
                // Notify the reporter that a new checkout has been queued.
                let reporter_id = self.reporter.as_mut().map(|reporter| {
                    reporter.on_git_checkout_queued(&RepositoryReference::from(&task.url))
                });

                entry.insert(PendingGitCheckout::Pending(reporter_id, vec![task.tx]));

                // Notify the reporter that the solve has started.
                if let Some((reporter, id)) = self.reporter.as_mut().zip(reporter_id) {
                    reporter.on_git_checkout_start(id)
                }

                let resolver = self.inner.git_resolver.clone();
                let client = self.inner.client.clone();
                let cache_dir = self.inner.cache_dir.clone();
                self.pending_futures.push(
                    async move {
                        let fetch = resolver
                            .fetch(task.url.clone(), client.clone(), cache_dir.clone(), None)
                            .await;
                        TaskResult::GitCheckedOut(task.url, fetch)
                    }
                    .boxed_local(),
                );
            }
        }
    }

    /// Called when a git checkout task has completed.
    fn on_git_checked_out(&mut self, url: GitUrl, result: Result<Fetch, GitError>) {
        let Some(PendingGitCheckout::Pending(reporter_id, pending)) =
            self.git_checkouts.get_mut(&url)
        else {
            unreachable!("cannot get a result for a git checkout that is not pending");
        };

        // Notify the reporter that the git checkout has finished.
        if let Some((reporter, id)) = self.reporter.as_mut().zip(*reporter_id) {
            reporter.on_git_checkout_finished(id)
        }

        match result {
            Ok(fetch) => {
                for tx in pending.drain(..) {
                    let _ = tx.send(Ok(fetch.clone()));
                }

                self.git_checkouts
                    .insert(url, PendingGitCheckout::CheckedOut(fetch));
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

                self.git_checkouts.insert(url, PendingGitCheckout::Errored);
            }
        }
    }
}
