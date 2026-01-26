use std::{collections::HashMap, time::Duration};

use indexmap::IndexMap;
use indicatif::{MultiProgress, ProgressBar};
use pixi_command_dispatcher::{ReporterContext, reporter::GitCheckoutId};
use pixi_git::{GIT_SSH_CLONING_WARNING_MSG, resolver::RepositoryReference, url::RepositoryUrl};
use pixi_progress::style_warning_pb;

/// A reporter implementation for source checkouts.
pub struct GitCheckoutProgress {
    /// The multi-progress bar. Usually, this is the global multi-progress bar.
    multi_progress: MultiProgress,
    /// The progress bar that is used as an anchor for placing other progress.
    anchor: ProgressBar,
    /// The id of the next checkout
    next_id: usize,
    /// A map of progress bars, by ID.
    bars: IndexMap<GitCheckoutId, ProgressBar>,
    /// References to the repository info
    repository_references: HashMap<GitCheckoutId, RepositoryReference>,
    /// Helper checkout progress bar for git SSH operations
    checkout_helper_pb: Option<(ProgressBar, usize)>,
}

impl GitCheckoutProgress {
    /// Creates a new source checkout reporter.
    pub fn new(multi_progress: MultiProgress, anchor: ProgressBar) -> Self {
        Self {
            multi_progress,
            anchor,
            next_id: 0,
            bars: Default::default(),
            repository_references: Default::default(),
            checkout_helper_pb: None,
        }
    }

    /// Returns a unique ID for a new progress bar.
    fn next_checkout_id(&mut self) -> GitCheckoutId {
        let id = GitCheckoutId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Similar to the default pixi_progress::default_progress_style, but with a
    /// spinner in front.
    pub fn spinner_style() -> indicatif::ProgressStyle {
        indicatif::ProgressStyle::with_template("  {spinner:.green} {prefix:30!} {wide_msg:.dim}")
            .expect("should be able to create a progress bar style")
    }

    /// Returns the repository reference for the given checkout ID.
    pub fn repo_reference(&self, id: GitCheckoutId) -> &RepositoryReference {
        self.repository_references
            .get(&id)
            .expect("the progress bar needs to be inserted for this checkout")
    }

    /// Returns the progress bar at the bottom
    pub fn last_progress_bar(&self) -> Option<&ProgressBar> {
        self.bars.last().map(|(_, pb)| pb)
    }

    /// Returns true if the specified URL refers to a checkout that might cause
    /// a hang if an SSH key with a passphrase is used.
    pub fn is_dangerous_ssh(url: &RepositoryUrl) -> bool {
        url.as_url().scheme().eq("ssh")
    }
}

impl pixi_command_dispatcher::GitCheckoutReporter for GitCheckoutProgress {
    /// Called when a git checkout was queued on the [`CommandQueue`].
    fn on_queued(
        &mut self,
        _context: Option<ReporterContext>,
        env: &RepositoryReference,
    ) -> GitCheckoutId {
        let checkout_id = self.next_checkout_id();
        self.repository_references.insert(checkout_id, env.clone());
        checkout_id
    }

    fn on_start(&mut self, checkout_id: GitCheckoutId) {
        let pb = self.multi_progress.insert_after(
            self.last_progress_bar().unwrap_or(&self.anchor),
            ProgressBar::hidden(),
        );
        let repo = self.repo_reference(checkout_id);
        pb.set_style(GitCheckoutProgress::spinner_style());
        pb.set_prefix("fetching git dependencies");
        pb.set_message(format!(
            "checking out {}@{}",
            repo.url.as_url(),
            repo.reference
        ));
        pb.enable_steady_tick(Duration::from_millis(100));

        if Self::is_dangerous_ssh(&repo.url) {
            match &mut self.checkout_helper_pb {
                Some((_, count)) => {
                    *count += 1;
                }
                None => {
                    let warning_pb = style_warning_pb(
                        self.multi_progress
                            .insert_before(&pb, ProgressBar::hidden()),
                        GIT_SSH_CLONING_WARNING_MSG.to_string(),
                    );
                    self.checkout_helper_pb = Some((warning_pb, 1));
                }
            }
        };

        self.bars.insert(checkout_id, pb);
    }

    fn on_finished(&mut self, checkout_id: GitCheckoutId) {
        let removed_pb = self
            .bars
            .shift_remove(&checkout_id)
            .expect("the progress bar needs to be inserted for this checkout");
        let repo = self.repo_reference(checkout_id);
        removed_pb.finish_with_message(format!(
            "checkout complete {}@{}",
            repo.url.as_url(),
            repo.reference
        ));
        removed_pb.finish_and_clear();

        if Self::is_dangerous_ssh(&repo.url) {
            let Some((pb, count)) = &mut self.checkout_helper_pb else {
                panic!("checkout helper progress bar should be present");
            };
            *count -= 1;
            if *count == 0 {
                pb.finish_and_clear();
                self.checkout_helper_pb = None;
            }
        }
    }
}
