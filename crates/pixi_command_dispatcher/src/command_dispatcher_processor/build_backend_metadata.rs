use std::{collections::hash_map::Entry, sync::Arc};

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use super::{CommandDispatcherProcessor, PendingDeduplicatingTask, TaskResult};
use crate::{
    BuildBackendMetadata, BuildBackendMetadataError, BuildBackendMetadataSpec,
    CommandDispatcherError, Reporter,
    command_dispatcher::{
        BuildBackendMetadataId, BuildBackendMetadataTask, CommandDispatcherContext,
    },
};

impl CommandDispatcherProcessor {
    /// Constructs a new [`BuildBackendMetadataId`] for the given `task`.
    fn gen_build_backend_metadata_id(
        &mut self,
        task: &BuildBackendMetadataTask,
    ) -> BuildBackendMetadataId {
        let id = BuildBackendMetadataId(self.build_backend_metadata_ids.len());
        self.build_backend_metadata_ids
            .insert(task.spec.clone(), id);
        if let Some(parent) = task.parent {
            self.parent_contexts.insert(id.into(), parent);
        }
        id
    }

    /// Called when a [`crate::command_dispatcher::BuildBackendMetadataTask`]
    /// task was received.
    pub(crate) fn on_build_backend_metadata(&mut self, task: BuildBackendMetadataTask) {
        // Lookup the id of the source metadata to avoid duplication.
        let source_metadata_id = {
            match self.build_backend_metadata_ids.get(&task.spec) {
                Some(id) => *id,
                None => self.gen_build_backend_metadata_id(&task),
            }
        };

        match self.build_backend_metadata.entry(source_metadata_id) {
            Entry::Occupied(mut entry) => match entry.get_mut() {
                PendingDeduplicatingTask::Pending(pending, _) => pending.push(task.tx),
                PendingDeduplicatingTask::Result(fetch, _) => {
                    let _ = task.tx.send(Ok(fetch.clone()));
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
                    .and_then(Reporter::as_build_backend_metadata_reporter)
                    .map(|reporter| reporter.on_queued(parent_context, &task.spec));

                if let Some(reporter_id) = reporter_id {
                    self.build_backend_metadata_reporters
                        .insert(source_metadata_id, reporter_id);
                }

                if let Some((reporter, reporter_id)) = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_build_backend_metadata_reporter)
                    .zip(reporter_id)
                {
                    reporter.on_started(reporter_id)
                }

                self.queue_build_backend_metadata_task(
                    source_metadata_id,
                    task.spec,
                    task.cancellation_token,
                );
            }
        }
    }

    /// Queues a [`BuildBackendMetadata`] task to be processed.
    fn queue_build_backend_metadata_task(
        &mut self,
        build_backend_metadata_id: BuildBackendMetadataId,
        spec: BuildBackendMetadataSpec,
        cancellation_token: CancellationToken,
    ) {
        let dispatcher = self.create_task_command_dispatcher(
            CommandDispatcherContext::BuildBackendMetadata(build_backend_metadata_id),
        );
        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(spec.request(dispatcher))
                .map(move |result| {
                    TaskResult::BuildBackendMetadata(
                        build_backend_metadata_id,
                        result.map_or(Err(CommandDispatcherError::Cancelled), |result| {
                            result.map(Arc::new)
                        }),
                    )
                })
                .boxed_local(),
        );
    }

    /// Called when a [`super::TaskResult::BuildBackendMetadata`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`super::CommandDispatcher`] that issues it.
    pub(crate) fn on_build_backend_metadata_result(
        &mut self,
        id: BuildBackendMetadataId,
        result: Result<
            Arc<BuildBackendMetadata>,
            CommandDispatcherError<BuildBackendMetadataError>,
        >,
    ) {
        self.parent_contexts.remove(&id.into());
        if let Some((reporter, reporter_id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_build_backend_metadata_reporter)
            .zip(self.build_backend_metadata_reporters.remove(&id))
        {
            reporter.on_finished(reporter_id);
        }

        self.build_backend_metadata
            .get_mut(&id)
            .expect("cannot find pending build backend metadata task")
            .on_pending_result(result)
    }
}
