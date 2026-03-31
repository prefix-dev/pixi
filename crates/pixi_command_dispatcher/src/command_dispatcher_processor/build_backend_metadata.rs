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

        match self.build_backend_metadata.on_task(
            task.spec.clone(),
            task.tx,
            BuildBackendMetadataId,
        ) {
            DedupAction::AlreadyCompleted => {}
            DedupAction::New {
                cancellation_token,
                dedup_group_id,
                id,
                ..
            } => {
                let dispatcher_context = CommandDispatcherContext::BuildBackendMetadata(id);
                if let Some(parent) = task.parent {
                    self.parent_contexts.insert(dispatcher_context, parent);
                }

                // Notify the reporter that a new task has been queued.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_build_backend_metadata_reporter)
                    .map(|reporter| reporter.on_queued(parent_context, &task.spec, dedup_group_id));

                if let Some(reporter_id) = reporter_id {
                    self.build_backend_metadata_reporters
                        .entry(id)
                        .or_default()
                        .push(reporter_id);
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
                                    result
                                        .map_or(Err(CommandDispatcherError::Cancelled), |result| {
                                            result.map(Arc::new)
                                        }),
                                ),
                            )
                        })
                        .boxed_local(),
                );
                self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
            }
            DedupAction::Subscribed {
                dedup_group_id, id, ..
            } => {
                let dispatcher_context = CommandDispatcherContext::BuildBackendMetadata(id);
                // Notify the reporter for the subscriber as well.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_build_backend_metadata_reporter)
                    .map(|reporter| reporter.on_queued(parent_context, &task.spec, dedup_group_id));

                if let Some(reporter_id) = reporter_id {
                    self.build_backend_metadata_reporters
                        .entry(id)
                        .or_default()
                        .push(reporter_id);
                }

                // Subscribers don't get the output stream.
                if let Some((reporter, reporter_id)) = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_build_backend_metadata_reporter)
                    .zip(reporter_id)
                {
                    reporter.on_started(reporter_id, Box::new(futures::stream::empty()))
                }
                self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
            }
        };
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

        let failed = result.is_err();
        self.build_backend_metadata.on_result(id, result);
        if let Some(reporter_ids) = self.build_backend_metadata_reporters.remove(&id)
            && let Some(reporter) = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_build_backend_metadata_reporter)
        {
            for reporter_id in reporter_ids {
                reporter.on_finished(reporter_id, failed);
            }
        }
    }
}
