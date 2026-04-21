use futures::FutureExt;
use pixi_path::AbsPathBuf;

use super::{CommandDispatcherProcessor, NewDedupTask, TaskResult};
use crate::{
    CommandDispatcherError,
    command_dispatcher::{
        CommandDispatcherContext, UrlCheckoutId,
        url::{UrlCheckout, UrlCheckoutTask},
    },
    reporter::Reportable,
};

impl CommandDispatcherProcessor {
    /// Called when a [`ForegroundMessage::UrlCheckout`] task was received.
    pub(crate) fn on_checkout_url(&mut self, task: UrlCheckoutTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        let action = self
            .url_checkouts
            .on_task(task.spec.clone(), task.tx, UrlCheckoutId);
        let parent_reporter_context = task.parent.and_then(|ctx| self.reporter_context(ctx));

        let Some(NewDedupTask {
            id,
            cancellation_token,
            ..
        }) = Self::start_dedup_task(
            self,
            action,
            &task.spec,
            task.parent,
            task.cancellation_token,
            parent_reporter_context,
            CommandDispatcherContext::UrlCheckout,
        )
        else {
            return;
        };

        if let Some(reporter_id) = self
            .url_checkout_reporters
            .get(&id)
            .and_then(|ids| ids.last().copied())
        {
            pixi_spec::UrlSpec::report_started(&self.reporter, reporter_id);
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
    }
}
