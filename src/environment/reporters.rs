use std::{sync::Arc, time::Duration};

use indicatif::ProgressBar;
use parking_lot::Mutex;
use pixi_build_frontend::CondaBuildReporter;

use crate::build::BuildReporter;

pub(super) struct CondaBuildProgress {
    main_progress: ProgressBar,
    build_progress: Mutex<Vec<(String, ProgressBar)>>,
}

impl CondaBuildProgress {
    pub(super) fn new(num_packages: u64) -> Self {
        // Create a new progress bar.
        let pb = ProgressBar::hidden();
        pb.set_length(num_packages);
        let pb = pixi_progress::global_multi_progress().add(pb);
        pb.set_style(pixi_progress::default_progress_style());
        // Building the package
        pb.set_prefix("building packages");
        pb.enable_steady_tick(Duration::from_millis(100));

        Self {
            main_progress: pb,
            build_progress: Mutex::new(Vec::default()),
        }
    }
}

impl CondaBuildProgress {
    /// Associate a progress bar with a build identifier, and get a build id
    /// back
    pub(super) fn associate(&self, identifier: &str) -> usize {
        let mut locked = self.build_progress.lock();
        let after = if locked.is_empty() {
            &self.main_progress
        } else {
            &locked
                .last()
                .expect("we just checked that `locked` isn't empty")
                .1
        };

        let pb = pixi_progress::global_multi_progress().insert_after(after, ProgressBar::hidden());

        locked.push((identifier.to_owned(), pb));
        locked.len() - 1
    }

    pub(super) fn end_progress_for(&self, build_id: usize, alternative_message: Option<String>) {
        self.main_progress.inc(1);
        if self.main_progress.position()
            == self
                .main_progress
                .length()
                .expect("expected length to be set for progress")
        {
            self.main_progress.finish_and_clear();
            // Clear all the build progress bars
            for (_, pb) in self.build_progress.lock().iter() {
                pb.finish_and_clear();
            }
            return;
        }
        let locked = self.build_progress.lock();

        // Finish the build progress bar
        let (identifier, pb) = locked.get(build_id).expect("build id should exist");
        // If there is an alternative message, use that
        let msg = if let Some(msg) = alternative_message {
            pb.set_style(
                indicatif::ProgressStyle::with_template("    {msg}")
                    .expect("should be able to create a progress bar style"),
            );
            msg
        } else {
            // Otherwise show the default message
            pb.set_style(
                indicatif::ProgressStyle::with_template("    {msg} in {elapsed}")
                    .expect("should be able to create a progress bar style"),
            );
            "built".to_string()
        };
        pb.finish_with_message(format!("✔ {msg}: {identifier}"));
    }
}

impl CondaBuildReporter for CondaBuildProgress {
    fn on_build_start(&self, build_id: usize) -> usize {
        // Actually show the progress
        let locked = self.build_progress.lock();
        let (identifier, pb) = locked.get(build_id).expect("build id should exist");
        let template =
            indicatif::ProgressStyle::with_template("    {spinner:.green} {msg} {elapsed}")
                .expect("should be able to create a progress bar style");
        pb.set_style(template);
        pb.set_message(format!("building {identifier}"));
        pb.enable_steady_tick(Duration::from_millis(100));
        // We keep operation and build id the same
        build_id
    }

    fn on_build_end(&self, operation: usize) {
        self.end_progress_for(operation, None);
    }

    fn on_build_output(&self, _operation: usize, line: String) {
        self.main_progress.suspend(|| eprintln!("{}", line));
    }
}

impl BuildReporter for CondaBuildProgress {
    fn on_build_cached(&self, build_id: usize) {
        self.end_progress_for(build_id, Some("cached".to_string()));
    }

    fn as_conda_build_reporter(self: Arc<Self>) -> Arc<dyn CondaBuildReporter> {
        self.clone()
    }
}
