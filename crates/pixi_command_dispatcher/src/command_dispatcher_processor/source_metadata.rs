use std::sync::Arc;

use futures::FutureExt;

use super::CommandDispatcherProcessor;
use super::TaskResult;
use super::dedup::DedupAction;
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

        // Since the key is always unique, this is always New.
        let DedupAction::New {
            id,
            cancellation_token,
            dedup_group_id,
        } = action
        else {
            unreachable!("source metadata tasks use unique keys");
        };

        let dispatcher_context = CommandDispatcherContext::SourceMetadata(id);

        if let Some(parent) = task.parent {
            self.parent_contexts.insert(dispatcher_context, parent);
        }

        // Notify the reporter.
        let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
        let reporter_id =
            task.spec
                .report_queued(&mut self.reporter, parent_context, Some(dedup_group_id));

        if let Some(reporter_id) = reporter_id {
            self.source_metadata_reporters
                .entry(id)
                .or_default()
                .push(reporter_id);
        }

        if let Some(reporter_id) = reporter_id {
            SourceMetadataSpec::report_started(&mut self.reporter, reporter_id);
        }

        let dispatcher = self.create_task_command_dispatcher(dispatcher_context);

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

        self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
    }
}
