use std::collections::hash_map::Entry;

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use super::{CommandDispatcherProcessor, PendingDeduplicatingTask, TaskResult};
use crate::{
    CommandDispatcherError, DevSourceMetadata, DevSourceMetadataError, DevSourceMetadataSpec,
    command_dispatcher::{CommandDispatcherContext, DevSourceMetadataId, DevSourceMetadataTask},
};

impl CommandDispatcherProcessor {
    /// Constructs a new [`DevSourceMetadataId`] for the given `task`.
    fn gen_dev_source_metadata_id(&mut self, task: &DevSourceMetadataTask) -> DevSourceMetadataId {
        let id = DevSourceMetadataId(self.dev_source_metadata_ids.len());
        self.dev_source_metadata_ids.insert(task.spec.clone(), id);
        if let Some(parent) = task.parent {
            self.parent_contexts.insert(id.into(), parent);
        }
        id
    }

    /// Called when a [`crate::command_dispatcher::DevSourceMetadataTask`]
    /// task was received.
    pub(crate) fn on_dev_source_metadata(&mut self, task: DevSourceMetadataTask) {
        // Lookup the id of the request to avoid duplication.
        let dev_source_metadata_id = {
            match self.dev_source_metadata_ids.get(&task.spec) {
                Some(id) => {
                    // We already have a pending task. Let's make sure that we are not trying to
                    // resolve the same thing in a cycle.
                    if self.contains_cycle(*id, task.parent) {
                        let _ = task.tx.send(Err(DevSourceMetadataError::Cycle));
                        return;
                    }

                    *id
                }
                None => self.gen_dev_source_metadata_id(&task),
            }
        };

        match self.dev_source_metadata.entry(dev_source_metadata_id) {
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

                self.queue_dev_source_metadata_task(
                    dev_source_metadata_id,
                    task.spec,
                    task.cancellation_token,
                    task.parent,
                );
            }
        }
    }

    /// Queues a dev source metadata task to be executed.
    fn queue_dev_source_metadata_task(
        &mut self,
        dev_source_metadata_id: DevSourceMetadataId,
        spec: DevSourceMetadataSpec,
        cancellation_token: CancellationToken,
        parent: Option<CommandDispatcherContext>,
    ) {
        let dispatcher_context =
            CommandDispatcherContext::DevSourceMetadata(dev_source_metadata_id);
        let dispatcher = self.create_task_command_dispatcher(dispatcher_context);

        // Create a child cancellation token linked to parent's token (if any).
        let cancellation_token = self.get_child_cancellation_token(parent, cancellation_token);

        // Store the cancellation token for this context so child tasks can link to it.
        self.store_cancellation_token(dispatcher_context, cancellation_token.clone());

        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(spec.request(dispatcher))
                .map(move |result| {
                    TaskResult::DevSourceMetadata(
                        dev_source_metadata_id,
                        Box::new(result.unwrap_or(Err(CommandDispatcherError::Cancelled))),
                    )
                })
                .boxed_local(),
        );
    }

    /// Called when a [`TaskResult::DevSourceMetadata`] task was received.
    ///
    /// This function will relay the result of the task back to the
    /// [`crate::CommandDispatcher`] that issues it.
    pub(crate) fn on_dev_source_metadata_result(
        &mut self,
        id: DevSourceMetadataId,
        result: Result<DevSourceMetadata, CommandDispatcherError<DevSourceMetadataError>>,
    ) {
        let context = CommandDispatcherContext::DevSourceMetadata(id);
        self.parent_contexts.remove(&context);
        self.remove_cancellation_token(context);

        self.dev_source_metadata
            .get_mut(&id)
            .expect("cannot find pending task")
            .on_pending_result(result);
    }
}
