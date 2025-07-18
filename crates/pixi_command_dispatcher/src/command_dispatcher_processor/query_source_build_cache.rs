use std::{collections::hash_map::Entry, sync::Arc};

use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingDeduplicatingTask, TaskResult};
use crate::{
    CommandDispatcherError, QuerySourceBuildCacheError, SourceBuildCacheEntry,
    command_dispatcher::{
        CommandDispatcherContext, QuerySourceBuildCacheId, QuerySourceBuildCacheTask,
    },
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::QuerySourceBuildCacheTask`]
    /// task was received.
    pub(crate) fn on_query_source_build_cache(&mut self, task: QuerySourceBuildCacheTask) {
        // Lookup the id of the request to avoid duplication.
        let query_source_build_id = {
            match self.query_source_build_cache_ids.get(&task.spec) {
                Some(id) => *id,
                None => {
                    // If the source build is not in the map we need to create a new id for it.
                    let id = QuerySourceBuildCacheId(self.query_source_build_cache_ids.len());
                    self.query_source_build_cache_ids
                        .insert(task.spec.clone(), id);
                    if let Some(parent) = task.parent {
                        self.parent_contexts.insert(id.into(), parent);
                    }
                    id
                }
            }
        };

        match self.query_source_build_cache.entry(query_source_build_id) {
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
    pub(crate) fn on_query_source_build_cache_result(
        &mut self,
        id: QuerySourceBuildCacheId,
        result: Result<SourceBuildCacheEntry, CommandDispatcherError<QuerySourceBuildCacheError>>,
    ) {
        self.parent_contexts.remove(&id.into());
        self.query_source_build_cache
            .get_mut(&id)
            .expect("cannot find pending task")
            .on_pending_result(result.map(Arc::new));
    }
}
