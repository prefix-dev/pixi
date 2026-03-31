use std::sync::Arc;

use futures::FutureExt;
use rattler_repodata_gateway::RunExportsReporter;

use super::CommandDispatcherProcessor;
use super::TaskResult;
use super::dedup::DedupAction;
use crate::{
    CommandDispatcherError, Reporter, SourceBuildError, SourceBuildResult,
    command_dispatcher::{CommandDispatcherContext, SourceBuildId, SourceBuildTask},
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::SourceBuildTask`]
    /// task was received.
    pub(crate) fn on_source_build(&mut self, task: SourceBuildTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        match self
            .source_build
            .on_task(task.spec.clone(), task.tx, SourceBuildId)
        {
            DedupAction::AlreadyCompleted => {}
            DedupAction::New {
                cancellation_token,
                dedup_group_id,
                id,
                ..
            } => {
                let dispatcher_context = CommandDispatcherContext::SourceBuild(id);
                if let Some(parent) = task.parent {
                    self.parent_contexts.insert(dispatcher_context, parent);
                }

                // Notify the reporter that a new task has been queued.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_build_reporter)
                    .map(|reporter| reporter.on_queued(parent_context, &task.spec, dedup_group_id));

                if let Some(reporter_id) = reporter_id {
                    self.source_build_reporters
                        .entry(id)
                        .or_default()
                        .push(reporter_id);
                }

                let dispatcher = self.create_task_command_dispatcher(dispatcher_context);
                let reporter_context = self.reporter_context(dispatcher_context);
                let (tx, rx) = futures::channel::mpsc::unbounded::<String>();

                let mut run_exports_reporter: Option<Arc<dyn RunExportsReporter>> = None;
                if let Some(reporter) = self.reporter.as_mut() {
                    let created = reporter.create_run_exports_reporter(reporter_context);
                    if let Some((source_reporter, reporter_id)) =
                        reporter.as_source_build_reporter().zip(
                            self.source_build_reporters
                                .get(&id)
                                .and_then(|ids| ids.first().copied()),
                        )
                    {
                        source_reporter.on_started(reporter_id, Box::new(rx));
                    }
                    run_exports_reporter = created;
                }

                self.pending_futures.push(
                    cancellation_token
                        .run_until_cancelled_owned(task.spec.build(
                            dispatcher,
                            run_exports_reporter.clone(),
                            tx,
                        ))
                        .map(move |result| {
                            TaskResult::SourceBuild(
                                id,
                                Box::new(result.unwrap_or(Err(CommandDispatcherError::Cancelled))),
                            )
                        })
                        .boxed_local(),
                );
                self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
            }
            DedupAction::Subscribed {
                dedup_group_id, id, ..
            } => {
                let dispatcher_context = CommandDispatcherContext::SourceBuild(id);
                // Notify the reporter for the subscriber as well.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_build_reporter)
                    .map(|reporter| reporter.on_queued(parent_context, &task.spec, dedup_group_id));

                if let Some(reporter_id) = reporter_id {
                    self.source_build_reporters
                        .entry(id)
                        .or_default()
                        .push(reporter_id);
                }

                // Subscribers don't get the output stream.
                if let Some((reporter, reporter_id)) = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_build_reporter)
                    .zip(reporter_id)
                {
                    reporter.on_started(reporter_id, Box::new(futures::stream::empty()));
                }
                self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
            }
        };
    }

    /// Called when a [`TaskResult::SourceBuild`] task was received.
    pub(crate) fn on_source_build_result(
        &mut self,
        id: SourceBuildId,
        result: Result<SourceBuildResult, CommandDispatcherError<SourceBuildError>>,
    ) {
        self.parent_contexts
            .remove(&CommandDispatcherContext::SourceBuild(id));

        let failed = result.is_err();
        self.source_build.on_result(id, result);
        if let Some(reporter_ids) = self.source_build_reporters.remove(&id)
            && let Some(reporter) = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_source_build_reporter)
        {
            for reporter_id in reporter_ids {
                reporter.on_finished(reporter_id, failed);
            }
        }
    }
}
