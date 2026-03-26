use std::{collections::hash_map::Entry, sync::Arc};

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use super::{CommandDispatcherProcessor, PendingDeduplicatingTask, TaskResult};
use crate::{
    CommandDispatcherError, Reporter, ResolvedSourceRecord, SourceRecordError, SourceRecordSpec,
    command_dispatcher::{CommandDispatcherContext, SourceRecordId, SourceRecordTask},
    source_metadata::Cycle,
    source_record::SourceRecordDeduplicationKey,
};

impl CommandDispatcherProcessor {
    /// Constructs a new [`SourceRecordId`] for the given `task`.
    fn gen_source_record_id(
        &mut self,
        cache_key: &SourceRecordDeduplicationKey,
        parent: Option<CommandDispatcherContext>,
    ) -> SourceRecordId {
        let id = SourceRecordId(self.source_record_ids.len());
        self.source_record_ids.insert(cache_key.clone(), id);
        if let Some(parent) = parent {
            self.parent_contexts.insert(id.into(), parent);
        }
        id
    }

    /// Called when a [`crate::command_dispatcher::SourceRecordTask`]
    /// task was received.
    pub(crate) fn on_source_record(&mut self, task: SourceRecordTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        let cache_key = SourceRecordDeduplicationKey::new(&task.spec);

        // Deduplicate by cache key, with cycle detection.
        let source_record_id = {
            match self.source_record_ids.get(&cache_key) {
                Some(id) => {
                    if self.contains_cycle(*id, task.parent) {
                        let _ = task
                            .tx
                            .send(Err(SourceRecordError::Cycle(Cycle::default())));
                        return;
                    }
                    *id
                }
                None => self.gen_source_record_id(&cache_key, task.parent),
            }
        };

        match self.source_record.entry(source_record_id) {
            Entry::Occupied(mut entry) => match entry.get_mut() {
                PendingDeduplicatingTask::Pending(pending, _) => {
                    pending.push(task.tx);
                }
                PendingDeduplicatingTask::Completed(result, _) => {
                    let _ = task.tx.send(result.clone());
                }
            },
            Entry::Vacant(entry) => {
                entry.insert(PendingDeduplicatingTask::Pending(
                    vec![task.tx],
                    task.parent,
                ));

                // Notify the reporter that a new source record resolve has been queued.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_record_reporter)
                    .map(|reporter| reporter.on_queued(parent_context, &task.spec));

                if let Some(reporter_id) = reporter_id {
                    self.source_record_reporters
                        .insert(source_record_id, reporter_id);
                }

                if let Some((reporter, reporter_id)) = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_record_reporter)
                    .zip(reporter_id)
                {
                    reporter.on_started(reporter_id)
                }

                self.queue_source_record_task(
                    source_record_id,
                    task.spec,
                    task.cancellation_token,
                    task.parent,
                );
            }
        }
    }

    /// Queues a source record task to be executed.
    fn queue_source_record_task(
        &mut self,
        source_record_id: SourceRecordId,
        spec: SourceRecordSpec,
        cancellation_token: CancellationToken,
        parent: Option<CommandDispatcherContext>,
    ) {
        let dispatcher_context = CommandDispatcherContext::SourceRecord(source_record_id);
        let dispatcher = self.create_task_command_dispatcher(dispatcher_context);

        let reporter_context = self.reporter_context(dispatcher_context);
        let run_exports_reporter = self
            .reporter
            .as_mut()
            .and_then(|reporter| reporter.create_run_exports_reporter(reporter_context));

        // Create a child cancellation token linked to parent's token (if any).
        let cancellation_token = self.get_child_cancellation_token(parent, cancellation_token);

        // Store the cancellation token for this context so child tasks can link to it.
        self.store_cancellation_token(dispatcher_context, cancellation_token.clone());

        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(spec.request(dispatcher, run_exports_reporter))
                .map(move |result| {
                    TaskResult::SourceRecord(
                        source_record_id,
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

    /// Called when a [`super::TaskResult::SourceRecord`] task was
    /// received.
    pub(crate) fn on_source_record_result(
        &mut self,
        id: SourceRecordId,
        result: Result<Arc<ResolvedSourceRecord>, CommandDispatcherError<SourceRecordError>>,
    ) {
        let context = CommandDispatcherContext::SourceRecord(id);
        self.parent_contexts.remove(&context);
        self.remove_cancellation_token(context);

        if let Some((reporter, reporter_id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_source_record_reporter)
            .zip(self.source_record_reporters.remove(&id))
        {
            reporter.on_finished(reporter_id);
        }

        if !self
            .source_record
            .get_mut(&id)
            .expect("cannot find pending source record task")
            .on_pending_result(result)
        {
            self.source_record.remove(&id);
        }
    }
}
