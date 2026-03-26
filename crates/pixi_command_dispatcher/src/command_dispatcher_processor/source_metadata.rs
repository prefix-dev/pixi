use std::sync::Arc;

use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingDeduplicatingTask, TaskResult};
use crate::{
    CommandDispatcherError, Reporter, SourceMetadata, SourceMetadataError, SourceMetadataSpec,
    command_dispatcher::{CommandDispatcherContext, SourceMetadataId, SourceMetadataTask},
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::SourceMetadataTask`]
    /// task was received.
    ///
    /// `SourceMetadata` is not deduplicated at this level. Its underlying
    /// work fans out to deduplicated `SourceRecord` tasks.
    pub(crate) fn on_source_metadata(&mut self, task: SourceMetadataTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        // Generate a unique id for this task (no deduplication).
        let source_metadata_id = SourceMetadataId(self.source_metadata_id_counter);
        self.source_metadata_id_counter += 1;

        if let Some(parent) = task.parent {
            self.parent_contexts
                .insert(source_metadata_id.into(), parent);
        }

        self.source_metadata.insert(
            source_metadata_id,
            PendingDeduplicatingTask::Pending(vec![task.tx], task.parent),
        );

        // Notify the reporter.
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
            task.parent,
        );
    }

    /// Queues a source metadata task to be executed.
    fn queue_source_metadata_task(
        &mut self,
        source_metadata_id: SourceMetadataId,
        spec: SourceMetadataSpec,
        cancellation_token: tokio_util::sync::CancellationToken,
        parent: Option<CommandDispatcherContext>,
    ) {
        let dispatcher_context = CommandDispatcherContext::SourceMetadata(source_metadata_id);
        let dispatcher = self.create_task_command_dispatcher(dispatcher_context);

        // Create a child cancellation token linked to parent's token (if any).
        let cancellation_token = self.get_child_cancellation_token(parent, cancellation_token);

        // Store the cancellation token for this context so child tasks can link to it.
        self.store_cancellation_token(dispatcher_context, cancellation_token.clone());

        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(spec.request(dispatcher))
                .map(move |result| {
                    TaskResult::SourceMetadata(
                        source_metadata_id,
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

    /// Called when a [`super::TaskResult::SourceMetadata`] task was
    /// received.
    pub(crate) fn on_source_metadata_result(
        &mut self,
        id: SourceMetadataId,
        result: Result<Arc<SourceMetadata>, CommandDispatcherError<SourceMetadataError>>,
    ) {
        let context = CommandDispatcherContext::SourceMetadata(id);
        self.parent_contexts.remove(&context);
        self.remove_cancellation_token(context);

        if let Some((reporter, reporter_id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_source_metadata_reporter)
            .zip(self.source_metadata_reporters.remove(&id))
        {
            reporter.on_finished(reporter_id);
        }

        if !self
            .source_metadata
            .get_mut(&id)
            .expect("cannot find pending task")
            .on_pending_result(result)
        {
            self.source_metadata.remove(&id);
        }
    }
}
