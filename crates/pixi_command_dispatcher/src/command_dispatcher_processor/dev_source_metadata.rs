use futures::FutureExt;

use super::CommandDispatcherProcessor;
use super::TaskResult;
use super::dedup::DedupAction;
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

        match self
            .dev_source_metadata
            .on_task(task.spec.clone(), task.tx, DevSourceMetadataId)
        {
            DedupAction::AlreadyCompleted => {}
            DedupAction::New {
                cancellation_token,
                id,
                ..
            } => {
                let dispatcher_context = CommandDispatcherContext::DevSourceMetadata(id);
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
                self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
            }
            DedupAction::Subscribed { id, .. } => {
                let dispatcher_context = CommandDispatcherContext::DevSourceMetadata(id);
                self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
            }
        };
    }
}
