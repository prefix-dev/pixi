use std::sync::Arc;

use futures::FutureExt;

use super::CommandDispatcherProcessor;
use super::TaskResult;
use super::dedup::DedupAction;
use crate::{
    BuildBackendMetadata, BuildBackendMetadataError, CommandDispatcherError, Reporter,
    command_dispatcher::{
        BuildBackendMetadataId, BuildBackendMetadataTask, CommandDispatcherContext,
    },
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::BuildBackendMetadataTask`]
    /// task was received.
    pub(crate) fn on_build_backend_metadata(&mut self, task: BuildBackendMetadataTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        let action =
            self.build_backend_metadata
                .on_task(task.spec.clone(), task.tx, BuildBackendMetadataId);

        let id = match &action {
            DedupAction::New { id, .. } | DedupAction::Subscribed { id } => *id,
            DedupAction::AlreadyCompleted => return,
        };

        let dispatcher_context = CommandDispatcherContext::BuildBackendMetadata(id);

        if let DedupAction::New {
            cancellation_token, ..
        } = action
        {
            if let Some(parent) = task.parent {
                self.parent_contexts.insert(dispatcher_context, parent);
            }

            // Notify the reporter that a new task has been queued.
            let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
            let reporter_id = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_build_backend_metadata_reporter)
                .map(|reporter| reporter.on_queued(parent_context, &task.spec));

            if let Some(reporter_id) = reporter_id {
                self.build_backend_metadata_reporters
                    .insert(id, reporter_id);
            }

            // Open a channel to receive build output.
            let (log_sink, rx) = futures::channel::mpsc::unbounded();

            if let Some((reporter, reporter_id)) = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_build_backend_metadata_reporter)
                .zip(reporter_id)
            {
                reporter.on_started(reporter_id, Box::new(rx))
            }

            let dispatcher = self.create_task_command_dispatcher(dispatcher_context);

            self.pending_futures.push(
                cancellation_token
                    .run_until_cancelled_owned(task.spec.request(dispatcher, log_sink))
                    .map(move |result| {
                        TaskResult::BuildBackendMetadata(
                            id,
                            Box::new(
                                result.map_or(Err(CommandDispatcherError::Cancelled), |result| {
                                    result.map(Arc::new)
                                }),
                            ),
                        )
                    })
                    .boxed_local(),
            );
        }

        self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
    }

    /// Called when a [`super::TaskResult::BuildBackendMetadata`] task was
    /// received.
    pub(crate) fn on_build_backend_metadata_result(
        &mut self,
        id: BuildBackendMetadataId,
        result: Result<
            Arc<BuildBackendMetadata>,
            CommandDispatcherError<BuildBackendMetadataError>,
        >,
    ) {
        self.parent_contexts
            .remove(&CommandDispatcherContext::BuildBackendMetadata(id));

        if let Some((reporter, reporter_id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_build_backend_metadata_reporter)
            .zip(self.build_backend_metadata_reporters.remove(&id))
        {
            let failed = result.is_err();
            reporter.on_finished(reporter_id, failed);
        }

        self.build_backend_metadata.on_result(id, result);
    }
}
