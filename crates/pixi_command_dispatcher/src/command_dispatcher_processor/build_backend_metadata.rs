use std::sync::Arc;

use futures::FutureExt;

use super::{CommandDispatcherProcessor, NewDedupTask, TaskResult};
use crate::{
    CommandDispatcherError, Reporter,
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
            CommandDispatcherContext::BuildBackendMetadata,
        )
        else {
            return;
        };

        // Open a channel to receive build output.
        let (log_sink, rx) = futures::channel::mpsc::unbounded();

        if let Some((reporter, reporter_id)) = self
            .reporter
            .as_deref()
            .and_then(Reporter::as_build_backend_metadata_reporter)
            .zip(
                self.build_backend_metadata_reporters
                    .get(&id)
                    .and_then(|ids| ids.last().copied()),
            )
        {
            reporter.on_started(reporter_id, Box::new(rx))
        }

        let dispatcher = self.create_task_command_dispatcher(context);

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
}
