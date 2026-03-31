use futures::FutureExt;

use super::CommandDispatcherProcessor;
use super::TaskResult;
use super::dedup::DedupAction;
use crate::{
    CommandDispatcherError, DevSourceMetadata, DevSourceMetadataError,
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

        let id = match &action {
            DedupAction::New { id, .. } | DedupAction::Subscribed { id, .. } => *id,
            DedupAction::AlreadyCompleted => return,
        };

        let dispatcher_context = CommandDispatcherContext::DevSourceMetadata(id);

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

        self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
    }

    /// Called when a [`TaskResult::DevSourceMetadata`] task was received.
    pub(crate) fn on_dev_source_metadata_result(
        &mut self,
        id: DevSourceMetadataId,
        result: Result<DevSourceMetadata, CommandDispatcherError<DevSourceMetadataError>>,
    ) {
        self.parent_contexts
            .remove(&CommandDispatcherContext::DevSourceMetadata(id));

        self.dev_source_metadata.on_result(id, result);
    }
}
