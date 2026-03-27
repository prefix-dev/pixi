//! Generic deduplication registry for the command dispatcher processor.
//!
//! [`DedupTaskRegistry`] manages the lifecycle of deduplicated tasks: multiple
//! callers requesting the same work share a single computation. The result is
//! cached for future requests. The computation is cancelled only when all
//! callers have dropped their futures.

use std::{collections::HashMap, hash::Hash};

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::{CommandDispatcherError, CommandDispatcherErrorResultExt};

/// A shared, deduplicated computation.
///
/// Multiple callers can subscribe via [`oneshot::Sender`]s. The computation
/// runs once; all subscribers get the cloned result.
pub(crate) enum SharedTask<T, E> {
    /// Task is currently executing.
    Pending {
        /// Channels to notify when the task completes.
        waiters: Vec<oneshot::Sender<Result<T, E>>>,

        /// Independent cancellation token for this task's future. Not linked
        /// to any single caller — cancelled only when all subscribers are gone.
        cancellation_token: CancellationToken,

        /// Number of active (non-cancelled) subscribers. When this reaches
        /// zero the task's cancellation token is cancelled.
        active_subscribers: usize,
    },

    /// Task completed (success or domain error). Result is cached for future
    /// requests.
    Completed(Result<T, E>),
}

/// The result of submitting a task to a [`DedupTaskRegistry`].
pub(crate) enum DedupAction<Id> {
    /// First request for this key — a new task must be spawned using the
    /// provided cancellation token.
    New {
        id: Id,
        cancellation_token: CancellationToken,
    },

    /// Deduplicated — added as a subscriber to an existing pending task.
    Subscribed { id: Id },

    /// Result was already cached — sent immediately to the caller.
    AlreadyCompleted,
}

/// A generic registry for deduplicated tasks.
///
/// Each task type provides its own `Key` (deduplication key, e.g. a spec) and
/// `Id` (a lightweight numeric identifier). The registry manages the mapping
/// from keys to tasks, result fan-out, caching, and subscriber tracking.
pub(crate) struct DedupTaskRegistry<Key, Id, T, E> {
    key_to_id: HashMap<Key, Id>,
    tasks: HashMap<Id, SharedTask<T, E>>,
    next_id: usize,
}

impl<Key, Id, T, E> Default for DedupTaskRegistry<Key, Id, T, E> {
    fn default() -> Self {
        Self {
            key_to_id: HashMap::default(),
            tasks: HashMap::default(),
            next_id: 0,
        }
    }
}

impl<Key, Id, T, E> DedupTaskRegistry<Key, Id, T, E>
where
    Key: Eq + Hash,
    Id: Copy + Eq + Hash,
    T: Clone,
    E: Clone,
{
    /// Handle an incoming task request.
    ///
    /// - If this is the first request for the given key, creates a new entry
    ///   and returns [`DedupAction::New`] with the task's cancellation token.
    /// - If a task is already pending for this key, adds the sender as a
    ///   subscriber and returns [`DedupAction::Subscribed`].
    /// - If the task already completed, sends the cached result immediately
    ///   and returns [`DedupAction::AlreadyCompleted`].
    ///
    /// The `make_id` closure constructs a type-safe ID from a `usize`.
    pub fn on_task(
        &mut self,
        key: Key,
        tx: oneshot::Sender<Result<T, E>>,
        make_id: impl FnOnce(usize) -> Id,
    ) -> DedupAction<Id> {
        let id = match self.key_to_id.get(&key) {
            Some(&id) => id,
            None => {
                let id = make_id(self.next_id);
                self.next_id += 1;
                self.key_to_id.insert(key, id);

                let cancellation_token = CancellationToken::new();
                self.tasks.insert(
                    id,
                    SharedTask::Pending {
                        waiters: vec![tx],
                        cancellation_token: cancellation_token.clone(),
                        active_subscribers: 1,
                    },
                );

                return DedupAction::New {
                    id,
                    cancellation_token,
                };
            }
        };

        let Some(task) = self.tasks.get_mut(&id) else {
            // The key exists but the task was removed (e.g. cancelled).
            // Re-create as a new task.
            let cancellation_token = CancellationToken::new();
            self.tasks.insert(
                id,
                SharedTask::Pending {
                    waiters: vec![tx],
                    cancellation_token: cancellation_token.clone(),
                    active_subscribers: 1,
                },
            );
            return DedupAction::New {
                id,
                cancellation_token,
            };
        };

        match task {
            SharedTask::Pending {
                waiters,
                active_subscribers,
                ..
            } => {
                waiters.push(tx);
                *active_subscribers += 1;
                DedupAction::Subscribed { id }
            }
            SharedTask::Completed(result) => {
                let _ = tx.send(result.clone());
                DedupAction::AlreadyCompleted
            }
        }
    }

    /// Handle a completed task result.
    ///
    /// On success or domain error: clones the result to all waiting
    /// subscribers and caches it for future requests.
    ///
    /// On cancellation: drops all subscribers (they observe `Cancelled`)
    /// and removes the entry so future requests can re-trigger the task.
    ///
    /// Returns `true` if the result was a real outcome (success/error),
    /// `false` if the task was cancelled.
    pub fn on_result(&mut self, id: Id, result: Result<T, CommandDispatcherError<E>>) -> bool {
        let task = self
            .tasks
            .get_mut(&id)
            .expect("cannot find pending task for result");

        let SharedTask::Pending { waiters, .. } = task else {
            unreachable!("received result for a task that is not pending");
        };

        let Some(result) = result.into_ok_or_failed() else {
            // Cancelled — drop all senders, remove entry so future requests
            // re-trigger the task.
            waiters.clear();
            self.tasks.remove(&id);
            self.key_to_id.retain(|_, v| *v != id);
            return false;
        };

        // Clone and send to all waiting subscribers.
        for tx in std::mem::take(waiters) {
            let _ = tx.send(result.clone());
        }

        *task = SharedTask::Completed(result);
        true
    }

    /// Called when a subscriber's cancellation token fires (caller dropped
    /// their future). Decrements the active subscriber count and cancels the
    /// task if no subscribers remain.
    pub fn on_subscriber_cancelled(&mut self, id: Id) {
        let Some(SharedTask::Pending {
            active_subscribers,
            cancellation_token,
            ..
        }) = self.tasks.get_mut(&id)
        else {
            // Task already completed or was removed — nothing to do.
            return;
        };

        *active_subscribers = active_subscribers.saturating_sub(1);
        if *active_subscribers == 0 {
            cancellation_token.cancel();
        }
    }

    /// Look up the id for a given key, if one exists.
    pub fn get_id(&self, key: &Key) -> Option<Id> {
        self.key_to_id.get(key).copied()
    }

    /// Clears completed entries, preserving in-flight (pending) tasks.
    pub fn clear_completed(&mut self) {
        self.tasks
            .retain(|_, v| matches!(v, SharedTask::Pending { .. }));
    }
}
