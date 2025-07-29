use std::time::Duration;

use indicatif::ProgressBar;
use pixi_consts::consts;
use rattler_conda_types::Platform;

use crate::workspace::grouped_environment::GroupedEnvironmentName;

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
}
