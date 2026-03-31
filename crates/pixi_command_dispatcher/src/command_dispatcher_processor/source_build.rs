use std::sync::Arc;

use futures::FutureExt;
use rattler_repodata_gateway::RunExportsReporter;

use super::{CommandDispatcherProcessor, NewDedupTask, TaskResult};
use crate::{
    CommandDispatcherError,
    command_dispatcher::{CommandDispatcherContext, SourceBuildId, SourceBuildTask},
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::SourceBuildTask`]
    /// task was received.
    pub(crate) fn on_source_build(&mut self, task: SourceBuildTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        let action = self
            .source_build
            .on_task(task.spec.clone(), task.tx, SourceBuildId);
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
            CommandDispatcherContext::SourceBuild,
        )
        else {
            return;
        };

        let dispatcher = self.create_task_command_dispatcher(context);
        let reporter_context = self.reporter_context(context);
        let (tx, rx) = futures::channel::mpsc::unbounded::<String>();

        let mut run_exports_reporter: Option<Arc<dyn RunExportsReporter>> = None;
        if let Some(reporter) = self.reporter.as_mut() {
            let created = reporter.create_run_exports_reporter(reporter_context);
            if let Some((source_reporter, reporter_id)) = reporter.as_source_build_reporter().zip(
                self.source_build_reporters
                    .get(&id)
                    .and_then(|ids| ids.first().copied()),
            ) {
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
    }
}
