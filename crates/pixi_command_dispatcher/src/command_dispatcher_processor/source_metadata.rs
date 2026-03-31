use std::sync::Arc;

use futures::FutureExt;

use super::{CommandDispatcherProcessor, NewDedupTask, TaskResult};
use crate::{
    CommandDispatcherError, SourceMetadataSpec,
    command_dispatcher::{CommandDispatcherContext, SourceMetadataId, SourceMetadataTask},
    reporter::Reportable,
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::SourceMetadataTask`]
    /// task was received.
    ///
    /// `SourceMetadata` is not deduplicated at this level. Its underlying
    /// work fans out to deduplicated `SourceRecord` tasks. Each request
    /// uses a unique counter as key so it always creates a new task.
    pub(crate) fn on_source_metadata(&mut self, task: SourceMetadataTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        // Use a unique counter as key — no deduplication at this level.
        let unique_key = self.source_metadata_id_counter;
        self.source_metadata_id_counter += 1;

        let action = self
            .source_metadata
            .on_task(unique_key, task.tx, SourceMetadataId);
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
            CommandDispatcherContext::SourceMetadata,
        )
        else {
            unreachable!("source metadata tasks use unique keys");
        };

        if let Some(reporter_id) = self
            .source_metadata_reporters
            .get(&id)
            .and_then(|ids| ids.last().copied())
        {
            SourceMetadataSpec::report_started(&mut self.reporter, reporter_id);
        }

        let dispatcher = self.create_task_command_dispatcher(context);

        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(task.spec.request(dispatcher))
                .map(move |result| {
                    TaskResult::SourceMetadata(
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
