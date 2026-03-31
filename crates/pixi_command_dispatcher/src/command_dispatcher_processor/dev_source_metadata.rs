use futures::FutureExt;

use super::{CommandDispatcherProcessor, NewDedupTask, TaskResult};
use crate::{
    CommandDispatcherError, DevSourceMetadataError,
    command_dispatcher::{CommandDispatcherContext, DevSourceMetadataId, DevSourceMetadataTask},
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::DevSourceMetadataTask`]
    /// task was received.
    pub(crate) fn on_dev_source_metadata(&mut self, task: DevSourceMetadataTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        // Cycle detection: if we already have a pending task for this key,
        // check whether following the parent chain would create a cycle.
        if let Some(id) = self.dev_source_metadata.get_id(&task.spec)
            && self.contains_cycle(id, task.parent)
        {
            let _ = task.tx.send(Err(DevSourceMetadataError::Cycle));
            return;
        }

        let action =
            self.dev_source_metadata
                .on_task(task.spec.clone(), task.tx, DevSourceMetadataId);
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
            CommandDispatcherContext::DevSourceMetadata,
        )
        else {
            return;
        };

        let dispatcher = self.create_task_command_dispatcher(context);

        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(task.spec.request(dispatcher))
                .map(move |result| {
                    TaskResult::DevSourceMetadata(
                        id,
                        Box::new(result.unwrap_or(Err(CommandDispatcherError::Cancelled))),
                    )
                })
                .boxed_local(),
        );
    }
}
