use std::sync::Arc;

use futures::FutureExt;

use super::{CommandDispatcherProcessor, NewDedupTask, TaskResult};
use crate::{
    CommandDispatcherError, SourceRecordError, SourceRecordSpec,
    command_dispatcher::{CommandDispatcherContext, SourceRecordId, SourceRecordTask},
    reporter::Reportable,
    source_metadata::{Cycle, cycle::SourceCycleKey},
    source_record::SourceRecordDeduplicationKey,
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::SourceRecordTask`]
    /// task was received.
    pub(crate) fn on_source_record(&mut self, task: SourceRecordTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        // Cycle detection: walk the parent chain looking for an ancestor that
        // is already resolving the same `(package, manifest source)` pair.
        // This is independent of the dedup key so incidental spec differences
        // can't prevent us from detecting a cycle.
        let cycle_key = SourceCycleKey {
            package: task.spec.package.clone(),
            source: task.spec.backend_metadata.manifest_source.clone(),
        };
        if self.has_source_cycle(&cycle_key, task.parent) {
            let _ = task
                .tx
                .send(Err(SourceRecordError::Cycle(Cycle::default())));
            return;
        }

        let cache_key = SourceRecordDeduplicationKey::new(&task.spec);
        let action = self
            .source_record
            .on_task(cache_key, task.tx, SourceRecordId);
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
            CommandDispatcherContext::SourceRecord,
        )
        else {
            return;
        };

        self.active_source_requests.insert(context, cycle_key);

        if let Some(reporter_id) = self
            .source_record_reporters
            .get(&id)
            .and_then(|ids| ids.last().copied())
        {
            SourceRecordSpec::report_started(&mut self.reporter, reporter_id);
        }

        let dispatcher = self.create_task_command_dispatcher(context);
        let reporter_context = self.reporter_context(context);
        let run_exports_reporter = self
            .reporter
            .as_mut()
            .and_then(|reporter| reporter.create_run_exports_reporter(reporter_context));

        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(task.spec.request(dispatcher, run_exports_reporter))
                .map(move |result| {
                    TaskResult::SourceRecord(
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
