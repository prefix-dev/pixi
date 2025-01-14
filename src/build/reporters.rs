use std::{sync::Arc, time::Duration};

use indicatif::ProgressBar;
use pixi_build_frontend::{CondaBuildReporter, CondaMetadataReporter};

pub trait BuildMetadataReporter: CondaMetadataReporter {
    /// Reporters that the metadata has been cached.
    fn on_metadata_cached(&self, build_id: usize);

    /// Cast upwards
    fn as_conda_metadata_reporter(self: Arc<Self>) -> Arc<dyn CondaMetadataReporter>;
}

/// Noop implementation of the BuildMetadataReporter trait.
struct NoopBuildMetadataReporter;
impl CondaMetadataReporter for NoopBuildMetadataReporter {
    fn on_metadata_start(&self, _build_id: usize) -> usize {
        0
    }

    fn on_metadata_end(&self, _operation: usize) {}
}
impl BuildMetadataReporter for NoopBuildMetadataReporter {
    fn on_metadata_cached(&self, _build_id: usize) {}

    fn as_conda_metadata_reporter(self: Arc<Self>) -> Arc<dyn CondaMetadataReporter> {
        self
    }
}

pub trait BuildReporter: CondaBuildReporter {
    /// Reports that the build has been cached.
    fn on_build_cached(&self, build_id: usize);

    /// Cast upwards
    fn as_conda_build_reporter(self: Arc<Self>) -> Arc<dyn CondaBuildReporter>;
}

/// Noop implementation of the BuildReporter trait.
struct NoopBuildReporter;
impl CondaBuildReporter for NoopBuildReporter {
    fn on_build_start(&self, _build_id: usize) -> usize {
        0
    }

    fn on_build_end(&self, _operation: usize) {}

    fn on_build_output(&self, _operation: usize, _line: String) {}
}
impl BuildReporter for NoopBuildReporter {
    fn on_build_cached(&self, _build_id: usize) {}

    fn as_conda_build_reporter(self: Arc<Self>) -> Arc<dyn CondaBuildReporter> {
        self
    }
}

pub struct GitReporter {
    pb: ProgressBar,
}

impl GitReporter {
    pub fn new(original_progress: &ProgressBar, num_repositories: Option<u64>) -> Self {
        let pb = pixi_progress::global_multi_progress()
            .insert_after(original_progress, ProgressBar::hidden());

        pb.set_style(pixi_progress::default_progress_style());

        pb.set_prefix("fetching git deps");
        pb.enable_steady_tick(Duration::from_millis(100));
        if let Some(num_repositories) = num_repositories {
            pb.set_length(num_repositories);
        }

        Self { pb }
    }

    /// Use this method to increment the progress bar
    /// It will also check if the progress bar is finished
    pub fn increment(&self) {
        self.pb.inc(1);
        self.check_finish();
    }

    /// Check if the progress bar is finished
    /// and clears it
    fn check_finish(&self) {
        if self.pb.position()
            == self
                .pb
                .length()
                .expect("expected length to be set for progress")
        {
            self.pb.set_message("");
            self.pb.finish_and_clear();
        }
    }
}

impl pixi_git::Reporter for GitReporter {
    fn on_checkout_start(&self, url: &url::Url, rev: &str) -> usize {
        self.pb.set_message(format!("checking out {}@{}", url, rev));
        0
    }

    fn on_checkout_complete(&self, url: &url::Url, rev: &str, _index: usize) {
        self.pb
            .set_message(format!("checkout complete {}@{}", url, rev));
        self.increment();
    }
}
