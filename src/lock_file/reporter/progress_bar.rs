use std::{
    borrow::Cow,
    fmt::Write,
    sync::{Arc},
    time::Duration,
};

use indicatif::{HumanBytes, ProgressBar, ProgressState};
use pixi_consts::consts;
use pypi_mapping::Reporter;
use rattler_conda_types::Platform;

use super::PurlAmendReporter;
use crate::project::grouped_environment::GroupedEnvironmentName;

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

        pb.set_style(indicatif::ProgressStyle::with_template("    {prefix:20!} ..").unwrap());
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
                .unwrap()
                .progress_chars("━━╾─"),
        );
    }

    pub(crate) fn set_bytes_update_style(&self, total: usize) {
        self.pb.set_length(total as u64);
        self.pb.set_position(0);
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(
                "  {spinner:.dim} {prefix:20!} [{elapsed_precise}] [{bar:20!.bright.yellow/dim.white}] {bytes:>8} @ {smoothed_bytes_per_sec:8} {msg:.dim}")
                .unwrap()
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
            .unwrap(),
        );
    }

    pub(crate) fn finish(&self) {
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(&format!(
                "  {} {{prefix:20!}} [{{elapsed_precise}}]",
                console::style(console::Emoji("✔", "↳")).green(),
            ))
            .unwrap(),
        );
        self.pb.finish_and_clear();
    }

    pub(crate) fn purl_amend_reporter(self: &Arc<Self>) -> Arc<dyn Reporter> {
        Arc::new(PurlAmendReporter::new(self.clone()))
    }
}
