use std::sync::Arc;

use futures::FutureExt;

use super::CommandDispatcherProcessor;
use super::TaskResult;
use super::dedup::DedupAction;
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
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        let cache_key = task.spec.key();

        // Cycle detection: if we already have a pending task for this key,
        // check whether following the parent chain would create a cycle.
        if let Some(id) = self.source_build_cache_status.get_id(&cache_key) {
            if self.contains_cycle(id, task.parent) {
                let _ = task.tx.send(Err(SourceBuildCacheStatusError::Cycle));
                return;
            }
        }

        let action = self.source_build_cache_status.on_task(
            cache_key,
            task.tx,
            SourceBuildCacheStatusId,
        );

        let id = match &action {
            DedupAction::New { id, .. } | DedupAction::Subscribed { id, .. } => *id,
            DedupAction::AlreadyCompleted => return,
        };

        let dispatcher_context = CommandDispatcherContext::QuerySourceBuildCache(id);

        if let DedupAction::New {
            cancellation_token, ..
        } = action
        {
            if let Some(parent) = task.parent {
                self.parent_contexts.insert(dispatcher_context, parent);
            }

            let dispatcher = self.create_task_command_dispatcher(dispatcher_context);

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

        // Push a monitoring future for this subscriber so the task is
        // cancelled when all callers drop their futures.
        self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
    }

    /// Called when a [`TaskResult::QuerySourceBuildCache`] task was
    /// received.
    pub(crate) fn on_source_build_cache_status_result(
        &mut self,
        id: SourceBuildCacheStatusId,
        result: Result<
            Arc<SourceBuildCacheEntry>,
            CommandDispatcherError<SourceBuildCacheStatusError>,
        >,
    ) {
        self.parent_contexts
            .remove(&CommandDispatcherContext::QuerySourceBuildCache(id));

        self.source_build_cache_status.on_result(id, result);
    }
}
