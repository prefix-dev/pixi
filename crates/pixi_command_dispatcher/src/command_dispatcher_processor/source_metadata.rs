use std::{collections::hash_map::Entry, sync::Arc};

use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingSourceMetadata, TaskResult};
use crate::{
    CommandDispatcherError, CommandDispatcherErrorResultExt,
    command_dispatcher::{CommandDispatcherContext, SourceMetadataId, SourceMetadataTask},
    source_metadata::{SourceMetadata, SourceMetadataError},
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::SourceMetadataTask`] task was
    /// received.
    pub(crate) fn on_source_metadata(&mut self, task: SourceMetadataTask) {
        // Lookup the id of the source metadata to avoid deduplication.
        let source_metadata_id = {
            match self.source_metadata_ids.get(&task.spec) {
                Some(id) => *id,
                None => {
                    // If the source metadata is not in the map, we need to
                    // create a new id for it.
                    let id = SourceMetadataId(self.source_metadata_ids.len());
                    self.source_metadata_ids.insert(task.spec.clone(), id);
                    id
                }
            }
        };

        match self.source_metadata.entry(source_metadata_id) {
            Entry::Occupied(mut entry) => match entry.get_mut() {
                PendingSourceMetadata::Pending(pending, _) => pending.push(task.tx),
                PendingSourceMetadata::Result(fetch, _) => {
                    let _ = task.tx.send(Ok(fetch.clone()));
                }
                PendingSourceMetadata::Errored => {
                    // Drop the sender, this will cause a cancellation on the other side.
                    drop(task.tx);
                }
            },
            Entry::Vacant(entry) => {
                entry.insert(PendingSourceMetadata::Pending(vec![task.tx], task.parent));

                let dispatcher = self.create_task_command_dispatcher(
                    CommandDispatcherContext::SourceMetadata(source_metadata_id),
                );
                self.pending_futures.push(
                    task.spec
                        .request(dispatcher)
                        .map(move |result| {
                            TaskResult::SourceMetadata(source_metadata_id, result.map(Arc::new))
                        })
                        .boxed_local(),
                );
            }
        }
    }

    /// Called when a [`super::TaskResult::SourceMetadata`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`super::CommandDispatcher`] that issues it.
    pub(crate) fn on_source_metadata_result(
        &mut self,
        id: SourceMetadataId,
        result: Result<Arc<SourceMetadata>, CommandDispatcherError<SourceMetadataError>>,
    ) {
        let Some(PendingSourceMetadata::Pending(pending, context)) =
            self.source_metadata.get_mut(&id)
        else {
            unreachable!("cannot get a result for source metadata that is not pending");
        };
        let context = *context;

        let Some(result) = result.into_ok_or_failed() else {
            // If the job was canceled, we can just drop the sending end
            // which will also cause a cancel on the receiving end.
            return;
        };

        match result {
            Ok(metadata) => {
                for tx in pending.drain(..) {
                    let _ = tx.send(Ok(metadata.clone()));
                }

                self.source_metadata
                    .insert(id, PendingSourceMetadata::Result(metadata, context));
            }
            Err(mut err) => {
                // Only send the error to the first channel, drop the rest, which cancels them.
                for tx in pending.drain(..) {
                    match tx.send(Err(err)) {
                        Ok(_) => return,
                        Err(Err(failed_to_send)) => err = failed_to_send,
                        Err(Ok(_)) => unreachable!(),
                    }
                }

                self.source_metadata
                    .insert(id, PendingSourceMetadata::Errored);
            }
        }
    }
}
