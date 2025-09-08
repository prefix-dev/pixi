use std::{collections::hash_map::Entry, sync::Arc};

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use super::{CommandDispatcherProcessor, PendingDeduplicatingTask, TaskResult};
use crate::{
    CommandDispatcherError, Reporter, SourceMetadata, SourceMetadataError, SourceMetadataSpec,
    command_dispatcher::{CommandDispatcherContext, SourceMetadataId, SourceMetadataTask},
    source_metadata::Cycle,
};

impl CommandDispatcherProcessor {
    /// Constructs a new [`SourceBuildId`] for the given `task`.
    fn gen_source_metadata_id(&mut self, task: &SourceMetadataTask) -> SourceMetadataId {
        let id = SourceMetadataId(self.source_metadata_ids.len());
        self.source_metadata_ids.insert(task.spec.clone(), id);
        if let Some(parent) = task.parent {
            self.parent_contexts.insert(id.into(), parent);
        }
        id
    }

    /// Called when a [`crate::command_dispatcher::SourceMetadataTask`]
    /// task was received.
    pub(crate) fn on_source_metadata(&mut self, task: SourceMetadataTask) {
        // Lookup the id of the source metadata to avoid deduplication.
        let source_metadata_id = {
            match self.source_metadata_ids.get(&task.spec) {
                Some(id) => {
                    // We already have a pending task for this source metadata. Let's make sure that
                    // we are not trying to resolve the same source metadata in a cycle.
                    if self.contains_cycle(*id, task.parent) {
                        let _ = task
                            .tx
                            .send(Err(SourceMetadataError::Cycle(Cycle::default())));
                        return;
                    }

                    *id
                }
                None => self.gen_source_metadata_id(&task),
            }
        };

        match self.source_metadata.entry(source_metadata_id) {
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

                // Notify the reporter that a new solve has been queued and started.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_metadata_reporter)
                    .map(|reporter| reporter.on_queued(parent_context, &task.spec));

                if let Some(reporter_id) = reporter_id {
                    self.source_metadata_reporters
                        .insert(source_metadata_id, reporter_id);
                }

                if let Some((reporter, reporter_id)) = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_metadata_reporter)
                    .zip(reporter_id)
                {
                    reporter.on_started(reporter_id)
                }

                self.queue_source_metadata_task(
                    source_metadata_id,
                    task.spec,
                    task.cancellation_token,
                );
            }
        }
    }

    /// Queues a source metadata task to be executed.
    fn queue_source_metadata_task(
        &mut self,
        source_metadata_id: SourceMetadataId,
        spec: SourceMetadataSpec,
        cancellation_token: CancellationToken,
    ) {
        let dispatcher = self.create_task_command_dispatcher(
            CommandDispatcherContext::SourceMetadata(source_metadata_id),
        );

        let dispatcher_context = CommandDispatcherContext::SourceMetadata(source_metadata_id);
        let reporter_context = self.reporter_context(dispatcher_context);
        let run_exports_reporter = self
            .reporter
            .as_mut()
            .and_then(|reporter| reporter.create_run_exports_reporter(reporter_context));

        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(spec.request(dispatcher, run_exports_reporter))
                .map(move |result| {
                    TaskResult::SourceMetadata(
                        source_metadata_id,
                        result.map_or(Err(CommandDispatcherError::Cancelled), |result| {
                            result.map(Arc::new)
                        }),
                    )
                })
                .boxed_local(),
        );
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
        if let Some((reporter, reporter_id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_source_metadata_reporter)
            .zip(self.source_metadata_reporters.remove(&id))
        {
            reporter.on_finished(reporter_id);
        }

        self.source_metadata
            .get_mut(&id)
            .expect("cannot find pending task")
            .on_pending_result(result)
    }
}
