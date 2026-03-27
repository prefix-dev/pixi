use futures::FutureExt;
use pixi_path::AbsPathBuf;

use super::CommandDispatcherProcessor;
use super::TaskResult;
use super::dedup::DedupAction;
use crate::{
    CommandDispatcherError, Reporter,
    command_dispatcher::{
        CommandDispatcherContext, UrlCheckoutId,
        url::{UrlCheckout, UrlCheckoutTask, UrlError},
    },
};

impl CommandDispatcherProcessor {
    /// Called when a [`ForegroundMessage::UrlCheckout`] task was received.
    pub(crate) fn on_checkout_url(&mut self, task: UrlCheckoutTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        let url = task.spec.url.clone();

        let action = self
            .url_checkouts
            .on_task(task.spec.clone(), task.tx, UrlCheckoutId);

        let id = match &action {
            DedupAction::New { id, .. } | DedupAction::Subscribed { id, .. } => *id,
            DedupAction::AlreadyCompleted => return,
        };

        let dispatcher_context = CommandDispatcherContext::UrlCheckout(id);

        if let DedupAction::New {
            cancellation_token,
            dedup_group_id,
            ..
        } = action
        {
            if let Some(parent) = task.parent {
                self.parent_contexts.insert(dispatcher_context, parent);
            }

            // Notify the reporter.
            let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
            let reporter_id = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_url_reporter)
                .map(|reporter| reporter.on_queued(parent_context, &url, dedup_group_id));

            if let Some(reporter_id) = reporter_id {
                self.url_checkout_reporters.insert(id, reporter_id);
            }

            if let Some((reporter, reporter_id)) = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_url_reporter)
                .zip(reporter_id)
            {
                reporter.on_start(reporter_id)
            }

            let resolver = self.inner.url_resolver.clone();
            let client = self.inner.download_client.clone();
            let cache_dir = self.inner.cache_dirs.url().clone();
            self.pending_futures.push(
                cancellation_token
                    .run_until_cancelled_owned(async move {
                        resolver
                            .fetch(task.spec, client, cache_dir.into_std_path_buf(), None)
                            .await
                            .map(|fetch| UrlCheckout {
                                pinned_url: fetch.pinned().clone(),
                                dir: AbsPathBuf::new(fetch.path())
                                    .expect("url fetch does not return absolute path")
                                    .into_assume_dir(),
                            })
                            .map_err(CommandDispatcherError::Failed)
                    })
                    .map(move |result| {
                        TaskResult::UrlCheckedOut(
                            id,
                            Box::new(result.unwrap_or(Err(CommandDispatcherError::Cancelled))),
                        )
                    })
                    .boxed_local(),
            );
        } else if let DedupAction::Subscribed { dedup_group_id, .. } = action {
            // Notify the reporter for the subscriber as well.
            let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
            let reporter_id = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_url_reporter)
                .map(|reporter| reporter.on_queued(parent_context, &url, dedup_group_id));

            if let Some(reporter_id) = reporter_id {
                self.url_checkout_reporters.insert(id, reporter_id);
            }

            if let Some((reporter, reporter_id)) = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_url_reporter)
                .zip(reporter_id)
            {
                reporter.on_start(reporter_id)
            }
        }

        self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
    }

    /// Called when a url checkout task has completed.
    pub(crate) fn on_url_checked_out(
        &mut self,
        id: UrlCheckoutId,
        result: Result<UrlCheckout, CommandDispatcherError<UrlError>>,
    ) {
        self.parent_contexts
            .remove(&CommandDispatcherContext::UrlCheckout(id));

        if let Some((reporter, reporter_id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_url_reporter)
            .zip(self.url_checkout_reporters.remove(&id))
        {
            reporter.on_finished(reporter_id)
        }

        self.url_checkouts.on_result(id, result);
    }
}
