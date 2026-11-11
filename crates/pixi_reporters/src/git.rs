use std::{collections::HashMap, sync::Arc, time::Duration};

use indexmap::IndexMap;
use indicatif::{MultiProgress, ProgressBar};
use parking_lot::Mutex;
use pixi_compute_reporters::{OperationId, OperationRegistry};
use pixi_git::{GIT_SSH_CLONING_WARNING_MSG, resolver::RepositoryReference, url::RepositoryUrl};
use pixi_progress::style_warning_pb;

struct GitCheckoutProgressInner {
    bars: IndexMap<OperationId, ProgressBar>,
    repository_references: HashMap<OperationId, RepositoryReference>,
    checkout_helper_pb: Option<(ProgressBar, usize)>,
}

/// A reporter implementation for source checkouts.
pub struct GitCheckoutProgress {
    registry: Arc<OperationRegistry>,
    /// The multi-progress bar. Usually, this is the global multi-progress bar.
    multi_progress: MultiProgress,
    /// The progress bar that is used as an anchor for placing other progress.
    anchor: ProgressBar,
    inner: Mutex<GitCheckoutProgressInner>,
}

impl GitCheckoutProgress {
    /// Creates a new source checkout reporter.
    pub fn new(
        registry: Arc<OperationRegistry>,
        multi_progress: MultiProgress,
        anchor: ProgressBar,
    ) -> Self {
        Self {
            registry,
            multi_progress,
            anchor,
            inner: Mutex::new(GitCheckoutProgressInner {
                bars: Default::default(),
                repository_references: Default::default(),
                checkout_helper_pb: None,
            }),
        }
    }

    /// Similar to the default pixi_progress::default_progress_style, but with a
    /// spinner in front.
    pub fn spinner_style() -> indicatif::ProgressStyle {
        indicatif::ProgressStyle::with_template("  {spinner:.green} {prefix:30!} {wide_msg:.dim}")
            .expect("should be able to create a progress bar style")
    }

    /// Returns true if the specified URL refers to a checkout that might cause
    /// a hang if an SSH key with a passphrase is used.
    pub fn is_dangerous_ssh(url: &RepositoryUrl) -> bool {
        url.as_url().scheme().eq("ssh")
    }
}

impl pixi_command_dispatcher::GitCheckoutReporter for GitCheckoutProgress {
    /// Called when a git checkout was queued on the command queue.
    fn on_queued(&self, env: &RepositoryReference) -> OperationId {
        let id = self.registry.allocate();
        self.inner
            .lock()
            .repository_references
            .insert(id, env.clone());
        id
    }

    fn on_started(&self, checkout_id: OperationId) {
        let mut inner = self.inner.lock();
        let repo = inner
            .repository_references
            .get(&checkout_id)
            .expect("the progress bar needs to be inserted for this checkout");
        let last_pb = inner.bars.last().map(|(_, pb)| pb).unwrap_or(&self.anchor);
        let pb = self
            .multi_progress
            .insert_after(last_pb, ProgressBar::hidden());
        pb.set_style(GitCheckoutProgress::spinner_style());
        pb.set_prefix("fetching git dependencies");
        pb.set_message(format!(
            "checking out {}@{}",
            repo.url.as_url(),
            repo.reference
        ));
        pb.enable_steady_tick(Duration::from_millis(100));

        if Self::is_dangerous_ssh(&repo.url) {
            match &mut inner.checkout_helper_pb {
                Some((_, count)) => {
                    *count += 1;
                }
                None => {
                    let warning_pb = style_warning_pb(
                        self.multi_progress
                            .insert_before(&pb, ProgressBar::hidden()),
                        GIT_SSH_CLONING_WARNING_MSG.to_string(),
                    );
                    inner.checkout_helper_pb = Some((warning_pb, 1));
                }
            }
        };

        inner.bars.insert(checkout_id, pb);
    }

    fn on_finished(&self, checkout_id: OperationId) {
        let mut inner = self.inner.lock();
        let removed_pb = inner
            .bars
            .shift_remove(&checkout_id)
            .expect("the progress bar needs to be inserted for this checkout");
        let repo = inner
            .repository_references
            .get(&checkout_id)
            .expect("the progress bar needs to be inserted for this checkout");
        removed_pb.finish_with_message(format!(
            "checkout complete {}@{}",
            repo.url.as_url(),
            repo.reference
        ));
        removed_pb.finish_and_clear();

        if Self::is_dangerous_ssh(&repo.url) {
            let Some((pb, count)) = &mut inner.checkout_helper_pb else {
                panic!("checkout helper progress bar should be present");
            };
            *count -= 1;
            if *count == 0 {
                pb.finish_and_clear();
                inner.checkout_helper_pb = None;
            }
        }
    }
}
