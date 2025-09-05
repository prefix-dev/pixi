use std::{collections::hash_map::Entry, sync::Arc};

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use super::{CommandDispatcherProcessor, PendingDeduplicatingTask, TaskResult};
use crate::{
    CommandDispatcherError, SourceBuildCacheEntry, SourceBuildCacheStatusError,
    SourceBuildCacheStatusSpec,
    command_dispatcher::{
        CommandDispatcherContext, SourceBuildCacheStatusId, SourceBuildCacheStatusTask,
    },
};

impl CommandDispatcherProcessor {
    /// Constructs a new [`SourceBuildCacheStatusId`] for the given `task`.
    fn gen_source_build_cache_status_id(
        &mut self,
        task: &SourceBuildCacheStatusTask,
    ) -> SourceBuildCacheStatusId {
        let id = SourceBuildCacheStatusId(self.source_build_cache_status_ids.len());
        self.source_build_cache_status_ids
            .insert(task.spec.clone(), id);
        if let Some(parent) = task.parent {
            self.parent_contexts.insert(id.into(), parent);
        }
        id
    }

    /// Called when a [`crate::command_dispatcher::SourceBuildCacheStatusTask`]
    /// task was received.
    pub(crate) fn on_source_build_cache_status(&mut self, task: SourceBuildCacheStatusTask) {
        // Lookup the id of the request to avoid duplication.
        let source_build_cache_status_id = {
            match self.source_build_cache_status_ids.get(&task.spec) {
                Some(id) => {
                    // We already have a pending task. Let's make sure that we are not trying to
                    // resolve the same thing in a cycle.
                    if self.contains_cycle(*id, task.parent) {
                        let _ = task.tx.send(Err(SourceBuildCacheStatusError::Cycle));
                        return;
                    }

                    *id
                }
                None => self.gen_source_build_cache_status_id(&task),
            }
        };

        match self
            .source_build_cache_status
            .entry(source_build_cache_status_id)
        {
            Entry::Occupied(mut entry) => match entry.get_mut() {
                PendingDeduplicatingTask::Pending(pending, _) => pending.push(task.tx),
                PendingDeduplicatingTask::Result(result, _) => {
                    let _ = task.tx.send(Ok(result.clone()));
                }
                PendingDeduplicatingTask::Errored => {
                    // Drop the sender, this will cause a cancellation on the other side.
                    drop(task.tx);
                }
            },
            Entry::Vacant(entry) => {
                entry.insert(PendingDeduplicatingTask::Pending(
                    vec![task.tx],
                    task.parent,
                ));

                self.queue_source_build_cache_status_task(
                    source_build_cache_status_id,
                    task.spec,
                    task.cancellation_token,
                );
            }
        }
    }

    /// Queues a source build cache status task to be executed.
    fn queue_source_build_cache_status_task(
        &mut self,
        source_build_cache_status_id: SourceBuildCacheStatusId,
        spec: SourceBuildCacheStatusSpec,
        cancellation_token: CancellationToken,
    ) {
        let dispatcher = self.create_task_command_dispatcher(
            CommandDispatcherContext::QuerySourceBuildCache(source_build_cache_status_id),
        );
        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(spec.query(dispatcher))
                .map(move |result| {
                    TaskResult::QuerySourceBuildCache(
                        source_build_cache_status_id,
                        result.unwrap_or(Err(CommandDispatcherError::Cancelled)),
                    )
                })
                .boxed_local(),
        );
    }

    /// Called when a [`TaskResult::QuerySourceBuildCache`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`crate::CommandDispatcher`] that issues it.
    pub(crate) fn on_source_build_cache_status_result(
        &mut self,
        id: SourceBuildCacheStatusId,
        result: Result<SourceBuildCacheEntry, CommandDispatcherError<SourceBuildCacheStatusError>>,
    ) {
        self.parent_contexts.remove(&id.into());
        self.source_build_cache_status
            .get_mut(&id)
            .expect("cannot find pending task")
            .on_pending_result(result.map(Arc::new));
    }
}
