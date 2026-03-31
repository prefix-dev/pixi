use std::sync::Arc;

use futures::FutureExt;

use super::CommandDispatcherProcessor;
use super::TaskResult;
use super::dedup::DedupAction;
use crate::{
    CommandDispatcherError, Reporter, ResolvedSourceRecord, SourceRecordError,
    command_dispatcher::{CommandDispatcherContext, SourceRecordId, SourceRecordTask},
    source_metadata::Cycle,
    source_record::SourceRecordDeduplicationKey,
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::SourceRecordTask`]
    /// task was received.
    pub(crate) fn on_source_record(&mut self, task: SourceRecordTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        let cache_key = SourceRecordDeduplicationKey::new(&task.spec);

        // Cycle detection: if we already have a pending task for this key,
        // check whether following the parent chain would create a cycle.
        if let Some(id) = self.source_record.get_id(&cache_key)
            && self.contains_cycle(id, task.parent)
        {
            let _ = task
                .tx
                .send(Err(SourceRecordError::Cycle(Cycle::default())));
            return;
        }

        match self
            .source_record
            .on_task(cache_key, task.tx, SourceRecordId)
        {
            DedupAction::AlreadyCompleted => {}
            DedupAction::New {
                cancellation_token,
                dedup_group_id,
                id,
                ..
            } => {
                let dispatcher_context = CommandDispatcherContext::SourceRecord(id);
                if let Some(parent) = task.parent {
                    self.parent_contexts.insert(dispatcher_context, parent);
                }

                // Notify the reporter.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_record_reporter)
                    .map(|reporter| reporter.on_queued(parent_context, &task.spec, dedup_group_id));

                if let Some(reporter_id) = reporter_id {
                    self.source_record_reporters
                        .entry(id)
                        .or_default()
                        .push(reporter_id);
                }

                if let Some((reporter, reporter_id)) = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_record_reporter)
                    .zip(reporter_id)
                {
                    reporter.on_started(reporter_id)
                }

                let dispatcher = self.create_task_command_dispatcher(dispatcher_context);
                let reporter_context = self.reporter_context(dispatcher_context);
                let run_exports_reporter = self
                    .reporter
                    .as_mut()
                    .and_then(|reporter| reporter.create_run_exports_reporter(reporter_context));

                self.pending_futures.push(
                    cancellation_token
                        .run_until_cancelled_owned(
                            task.spec.request(dispatcher, run_exports_reporter),
                        )
                        .map(move |result| {
                            TaskResult::SourceRecord(
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
                let dispatcher_context = CommandDispatcherContext::SourceRecord(id);
                // Notify the reporter for the subscriber as well.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_record_reporter)
                    .map(|reporter| reporter.on_queued(parent_context, &task.spec, dedup_group_id));

                if let Some(reporter_id) = reporter_id {
                    self.source_record_reporters
                        .entry(id)
                        .or_default()
                        .push(reporter_id);
                }

                if let Some((reporter, reporter_id)) = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_record_reporter)
                    .zip(reporter_id)
                {
                    reporter.on_started(reporter_id)
                }
                self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
            }
        };
    }

    /// Called when a [`super::TaskResult::SourceRecord`] task was received.
    pub(crate) fn on_source_record_result(
        &mut self,
        id: SourceRecordId,
        result: Result<Arc<ResolvedSourceRecord>, CommandDispatcherError<SourceRecordError>>,
    ) {
        self.parent_contexts
            .remove(&CommandDispatcherContext::SourceRecord(id));

        self.source_record.on_result(id, result);
        if let Some(reporter_ids) = self.source_record_reporters.remove(&id)
            && let Some(reporter) = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_source_record_reporter)
        {
            for reporter_id in reporter_ids {
                reporter.on_finished(reporter_id);
            }
        }
    }
}
