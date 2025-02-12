use std::{borrow::Cow, fmt::Write, sync::Arc, time::Duration};

use indicatif::{HumanBytes, ProgressBar, ProgressState};
use pixi_build_frontend::CondaMetadataReporter;
use pixi_consts::consts;
use pypi_mapping::Reporter;
use rattler_conda_types::Platform;

use super::PurlAmendReporter;
use crate::{build::BuildMetadataReporter, workspace::grouped_environment::GroupedEnvironmentName};

/// A helper struct that manages a progress-bar for solving an environment.
#[derive(Clone)]
pub(crate) struct SolveProgressBar {
    pub pb: ProgressBar,
}

impl SolveProgressBar {
    pub(crate) fn new(
        pb: ProgressBar,
        platform: Platform,
        environment_name: GroupedEnvironmentName,
    ) -> Self {
        let name_and_platform = format!(
            "{}:{}",
            environment_name.fancy_display(),
            consts::PLATFORM_STYLE.apply_to(platform)
        );

        pb.set_style(
            indicatif::ProgressStyle::with_template("    {prefix:20!} ..")
                .expect("should be able to create a progress bar style"),
        );
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.set_prefix(name_and_platform);
        Self { pb }
    }

    pub(crate) fn start(&self) {
        self.pb.reset_elapsed();
        self.reset_style()
    }

    pub(crate) fn set_message(&self, msg: impl Into<Cow<'static, str>>) {
        self.pb.set_message(msg);
    }

    pub(crate) fn inc(&self, n: u64) {
        self.pb.inc(n);
    }

    pub(crate) fn set_position(&self, n: u64) {
        self.pb.set_position(n)
    }

    pub(crate) fn set_update_style(&self, total: usize) {
        self.pb.set_length(total as u64);
        self.pb.set_position(0);
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(
                "  {spinner:.dim} {prefix:20!} [{elapsed_precise}] [{bar:20!.bright.yellow/dim.white}] {pos:>4}/{len:4} {msg:.dim}")
                .expect("should be able to create a progress bar style")
                .progress_chars("━━╾─"),
        );
    }

    pub(crate) fn set_bytes_update_style(&self, total: usize) {
        self.pb.set_length(total as u64);
        self.pb.set_position(0);
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(
                "  {spinner:.dim} {prefix:20!} [{elapsed_precise}] [{bar:20!.bright.yellow/dim.white}] {bytes:>8} @ {smoothed_bytes_per_sec:8} {msg:.dim}")
                .expect("should be able to create a progress bar style")
                .progress_chars("━━╾─")
                .with_key(
                    "smoothed_bytes_per_sec",
                    |s: &ProgressState, w: &mut dyn Write| match (s.pos(), s.elapsed().as_millis()) {
                        (pos, elapsed_ms) if elapsed_ms > 0 => {
                            write!(w, "{}/s", HumanBytes((pos as f64 * 1000_f64 / elapsed_ms as f64) as u64)).unwrap()
                        }
                        _ => write!(w, "-").unwrap(),
                    },
                )
        );
    }

    pub(crate) fn reset_style(&self) {
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(
                "  {spinner:.dim} {prefix:20!} [{elapsed_precise}] {msg:.dim}",
            )
            .expect("should be able to create a progress bar style"),
        );
    }

    pub(crate) fn finish(&self) {
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(&format!(
                "  {} {{prefix:20!}} [{{elapsed_precise}}]",
                console::style(console::Emoji("✔", "↳")).green(),
            ))
            .expect("should be able to create a progress bar style"),
        );
        self.pb.finish_and_clear();
    }

    pub(crate) fn purl_amend_reporter(self: &Arc<Self>) -> Arc<dyn Reporter> {
        Arc::new(PurlAmendReporter::new(self.clone()))
    }
}

/// Struct that manages the progress for getting source metadata.
pub(crate) struct CondaMetadataProgress {
    progress_bar: ProgressBar,
}

impl CondaMetadataProgress {
    /// Creates a new progress bar for the metadata, and activates it
    pub(crate) fn new(original_progress: &ProgressBar, num_packages: u64) -> Self {
        // Create a new progress bar.
        let pb = pixi_progress::global_multi_progress()
            .insert_after(original_progress, ProgressBar::hidden());
        pb.set_length(num_packages);
        pb.set_style(pixi_progress::default_progress_style());
        // Building the package
        pb.set_prefix("retrieving metadata");
        pb.enable_steady_tick(Duration::from_millis(100));
        Self { progress_bar: pb }
    }
}

impl CondaMetadataProgress {
    /// Use this method to increment the progress bar
    /// It will also check if the progress bar is finished
    pub fn increment(&self) {
        self.progress_bar.inc(1);
        self.check_finish();
    }

    /// Check if the progress bar is finished
    /// and clears it
    fn check_finish(&self) {
        if self.progress_bar.position()
            == self
                .progress_bar
                .length()
                .expect("expected length to be set for progress")
        {
            self.progress_bar.set_message("");
            self.progress_bar.finish_and_clear();
        }
    }
}

impl CondaMetadataReporter for CondaMetadataProgress {
    fn on_metadata_start(&self, _build_id: usize) -> usize {
        // Started metadata extraction
        self.progress_bar.set_message("extracting");
        0
    }

    fn on_metadata_end(&self, _operation: usize) {
        // Finished metadata extraction
        self.increment();
    }
}

// This is the same but for the cached variants
impl BuildMetadataReporter for CondaMetadataProgress {
    fn on_metadata_cached(&self, _build_id: usize) {
        self.increment();
    }

    fn as_conda_metadata_reporter(self: Arc<Self>) -> Arc<dyn CondaMetadataReporter> {
        self.clone()
    }
}
