use std::{collections::hash_map::Entry, sync::Arc};

use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingDeduplicatingTask, TaskResult};
use crate::{
    CommandDispatcherError, SourceBuildCacheEntry, SourceBuildCacheStatusError,
    command_dispatcher::{
        CommandDispatcherContext, SourceBuildCacheStatusId, SourceBuildCacheStatusTask,
    },
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::SourceBuildCacheStatusTask`]
    /// task was received.
    pub(crate) fn on_source_build_cache_status(&mut self, task: SourceBuildCacheStatusTask) {
        // Lookup the id of the request to avoid duplication.
        let query_source_build_id = {
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
                None => {
                    // If the source build is not in the map we need to create a new id for it.
                    let id = SourceBuildCacheStatusId(self.source_build_cache_status_ids.len());
                    self.source_build_cache_status_ids
                        .insert(task.spec.clone(), id);
                    if let Some(parent) = task.parent {
                        self.parent_contexts.insert(id.into(), parent);
                    }
                    id
                }
            }
        };

        match self.source_build_cache_status.entry(query_source_build_id) {
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

                // Add the task to the list of pending futures.
                let dispatcher_context =
                    CommandDispatcherContext::QuerySourceBuildCache(query_source_build_id);
                let dispatcher = self.create_task_command_dispatcher(dispatcher_context);
                self.pending_futures.push(
                    task.spec
                        .query(dispatcher)
                        .map(move |result| {
                            TaskResult::QuerySourceBuildCache(query_source_build_id, result)
                        })
                        .boxed_local(),
                );
            }
        }
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
