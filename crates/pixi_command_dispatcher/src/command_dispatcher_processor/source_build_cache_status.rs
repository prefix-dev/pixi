use std::sync::Arc;

use futures::FutureExt;

use super::{CommandDispatcherProcessor, NewDedupTask, TaskResult};
use crate::{
    CommandDispatcherError, SourceBuildCacheStatusError,
    command_dispatcher::{
        CommandDispatcherContext, SourceBuildCacheStatusId, SourceBuildCacheStatusTask,
    },
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::SourceBuildCacheStatusTask`]
    /// task was received.
    pub(crate) fn on_source_build_cache_status(&mut self, task: SourceBuildCacheStatusTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        let cache_key = task.spec.key();

        // Cycle detection: if we already have a pending task for this key,
        // check whether following the parent chain would create a cycle.
        if let Some(id) = self.source_build_cache_status.get_id(&cache_key)
            && self.contains_cycle(id, task.parent)
        {
            let _ = task.tx.send(Err(SourceBuildCacheStatusError::Cycle));
            return;
        }

        let action =
            self.source_build_cache_status
                .on_task(cache_key, task.tx, SourceBuildCacheStatusId);
        let parent_reporter_context = task.parent.and_then(|ctx| self.reporter_context(ctx));

        let Some(NewDedupTask {
            id,
            cancellation_token,
            context,
        }) = Self::start_dedup_task(
            self,
            action,
            &task.spec,
            task.parent,
            task.cancellation_token,
            parent_reporter_context,
            CommandDispatcherContext::QuerySourceBuildCache,
        )
        else {
            return;
        };

        let dispatcher = self.create_task_command_dispatcher(context);

        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(task.spec.query(dispatcher))
                .map(move |result| {
                    TaskResult::QuerySourceBuildCache(
                        id,
                        Box::new(
                            result
                                .unwrap_or(Err(CommandDispatcherError::Cancelled))
                                .map(Arc::new),
                        ),
                    )
                })
                .boxed_local(),
        );
    }
}
