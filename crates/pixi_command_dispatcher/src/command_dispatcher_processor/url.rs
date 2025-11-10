use std::collections::hash_map::Entry;

use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingUrlCheckout, PendingUrlWaiter, TaskResult};
use crate::{
    CommandDispatcherError, Reporter,
    command_dispatcher::url::{UrlCheckout, UrlCheckoutTask},
};
use pixi_spec::UrlSpec;
use pixi_url::UrlError;
use tokio_util::sync::CancellationToken;

impl CommandDispatcherProcessor {
    /// Called when a [`ForegroundMessage::UrlCheckout`] task was received.
    pub(crate) fn on_checkout_url(&mut self, task: UrlCheckoutTask) {
        let UrlCheckoutTask {
            spec,
            parent,
            tx,
            cancellation_token,
        } = task;
        let parent_context = parent.and_then(|ctx| self.reporter_context(ctx));
        let url = spec.url.clone();

        match self.url_checkouts.entry(url.clone()) {
            Entry::Occupied(mut existing_checkout) => {
                match existing_checkout.get_mut() {
                    PendingUrlCheckout::Pending(_, pending) => pending.push(PendingUrlWaiter {
                        spec: spec.clone(),
                        tx,
                    }),
                    PendingUrlCheckout::CheckedOut(fetch) => {
                        let _ = tx.send(Ok(fetch.clone()));
                    }
                    PendingUrlCheckout::Errored => {
                        // Drop the sender, this will cause a cancellation on the other side.
                        drop(tx)
                    }
                }
            }
            Entry::Vacant(entry) => {
                // Notify the reporter that a new checkout has been queued.
                let reporter_id = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_url_reporter)
                    .map(|reporter| reporter.on_queued(parent_context, &url));

                entry.insert(PendingUrlCheckout::Pending(
                    reporter_id,
                    vec![PendingUrlWaiter {
                        spec: spec.clone(),
                        tx,
                    }],
                ));

                // Notify the reporter that the fetch has started.
                if let Some((reporter, id)) = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_url_reporter)
                    .zip(reporter_id)
                {
                    reporter.on_start(id)
                }

                self.spawn_url_fetch(spec, cancellation_token, url);
            }
        }
    }

    fn spawn_url_fetch(
        &mut self,
        spec: UrlSpec,
        cancellation_token: CancellationToken,
        url: url::Url,
    ) {
        let resolver = self.inner.url_resolver.clone();
        let client = self.inner.download_client.clone();
        let cache_dir = self.inner.cache_dirs.url().clone();
        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(async move {
                    resolver
                        .fetch(spec, client, cache_dir, None)
                        .await
                        .map(|fetch| UrlCheckout {
                            pinned_url: fetch.pinned().clone(),
                            dir: fetch.path().to_path_buf(),
                        })
                        .map_err(CommandDispatcherError::Failed)
                })
                .map(|fetch| {
                    TaskResult::UrlCheckedOut(
                        url,
                        Box::new(fetch.unwrap_or(Err(CommandDispatcherError::Cancelled))),
                    )
                })
                .boxed_local(),
        );
    }

    /// Called when a url checkout task has completed.
    pub(crate) fn on_url_checked_out(
        &mut self,
        url: url::Url,
        result: Result<UrlCheckout, CommandDispatcherError<UrlError>>,
    ) {
        let Some(PendingUrlCheckout::Pending(reporter_id, pending)) =
            self.url_checkouts.get_mut(&url)
        else {
            unreachable!("cannot get a result for a url checkout that is not pending");
        };

        // Notify the reporter that the url checkout has finished.
        if let Some((reporter, id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_url_reporter)
            .zip(*reporter_id)
        {
            reporter.on_finished(id)
        }

        match result {
            Ok(fetch) => {
                for waiter in pending.drain(..) {
                    fulfill_waiter(waiter, &fetch);
                }

                // Store the fetch in the url map.
                self.url_checkouts
                    .insert(url, PendingUrlCheckout::CheckedOut(fetch));
            }
            Err(CommandDispatcherError::Failed(mut err)) => {
                // Only send the error to the first channel, drop the rest, which cancels them.
                for waiter in pending.drain(..) {
                    let PendingUrlWaiter { tx, .. } = waiter;
                    match tx.send(Err(err)) {
                        Ok(_) => return,
                        Err(Err(failed_to_send)) => err = failed_to_send,
                        Err(Ok(_)) => unreachable!(),
                    }
                }

                self.url_checkouts.insert(url, PendingUrlCheckout::Errored);
            }
            Err(CommandDispatcherError::Cancelled) => {
                self.url_checkouts.insert(url, PendingUrlCheckout::Errored);
            }
        }
    }
}

fn fulfill_waiter(waiter: PendingUrlWaiter, checkout: &UrlCheckout) {
    let PendingUrlWaiter { spec, tx } = waiter;
    let result = validate_checkout(&spec, checkout).map(|()| checkout.clone());
    let _ = tx.send(result);
}

#[allow(clippy::result_large_err)]
fn validate_checkout(spec: &UrlSpec, checkout: &UrlCheckout) -> Result<(), UrlError> {
    if let Some(expected) = spec.sha256 {
        let actual = checkout.pinned_url.sha256;
        if expected != actual {
            return Err(UrlError::Sha256Mismatch {
                url: spec.url.clone(),
                expected,
                actual,
            });
        }
    }

    if let Some(expected) = spec.md5 {
        let actual = checkout
            .pinned_url
            .md5
            .expect("URL checkouts always record md5 hashes");
        if actual != expected {
            return Err(UrlError::Md5Mismatch {
                url: spec.url.clone(),
                expected,
                actual,
            });
        }
    }

    Ok(())
}
