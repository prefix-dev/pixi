#![allow(clippy::unwrap_used)]
// Currently duplicates `rattler-build` module with the same name.
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use indicatif::{MultiProgress, ProgressBar, ProgressFinish, ProgressStyle};
use pixi_progress::ProgressBarPlacement;
use rattler::package_cache::CacheReporter;
use rattler_conda_types::RepoDataRecord;
use rattler_repodata_gateway::RunExportsReporter;

/// A reporter that makes it easy to show the progress of updating the package
/// cache.
#[derive(Clone)]
pub struct PackageCacheReporter {
    inner: Arc<Mutex<PackageCacheReporterInner>>,
}

impl PackageCacheReporter {
    /// Construct a new reporter.
    pub fn _new(multi_progress: MultiProgress, placement: ProgressBarPlacement) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PackageCacheReporterInner {
                multi_progress,
                placement,

                progress_chars: Cow::Borrowed("━━╾─"),
                prefix: Cow::Borrowed(""),

                validation_pb: None,
                download_pb: None,
                packages: Vec::new(),

                style_cache: HashMap::default(),
            })),
        }
    }

    /// Specify a prefix for the progress bars.
    pub fn _with_prefix(self, prefix: impl Into<Cow<'static, str>>) -> Self {
        let mut inner = self.inner.lock().unwrap();
        inner.prefix = prefix.into();
        inner.rerender();
        drop(inner);
        self
    }

    /// Adds a new package to the reporter. Returns a
    /// `PackageCacheReporterEntry` which can be passed to any of the cache
    /// functions of a package cache to track progress.
    pub fn add(&self, record: &RepoDataRecord) -> PackageCacheReporterEntry {
        let mut inner = self.inner.lock().unwrap();

        let entry = ProgressEntry {
            name: record.package_record.name.as_normalized().to_string(),
            size: record.package_record.size,
            validate_started: false,
            validate_completed: false,
            download_started: false,
            download_progress: None,
            download_completed: false,
        };

        let entry_idx = inner.packages.len();
        inner.packages.push(entry);

        drop(inner);

        PackageCacheReporterEntry {
            inner: self.inner.clone(),
            entry_idx,
        }
    }
}

impl RunExportsReporter for PackageCacheReporter {
    fn add(&self, record: &RepoDataRecord) -> Arc<dyn CacheReporter> {
        Arc::new(self.add(record))
    }
}

#[derive(Default)]
struct PackageCacheReporterInner {
    multi_progress: MultiProgress,
    placement: ProgressBarPlacement,

    prefix: Cow<'static, str>,
    progress_chars: Cow<'static, str>,

    validation_pb: Option<ProgressBar>,
    download_pb: Option<ProgressBar>,

    packages: Vec<ProgressEntry>,

    style_cache: HashMap<ProgressStyleProperties, ProgressStyle>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProgressStyleProperties {
    status: ProgressStatus,
    progress_type: ProgressType,
}

/// Defines the current status of a progress bar.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
enum ProgressStatus {
    /// The progress bar is showing active work.
    Active,

    /// The progress bar was active but has been paused for the moment.
    Paused,

    /// The progress bar finished.
    Finished,
}

/// Defines the type of progress-bar.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
enum ProgressType {
    Generic,
    Bytes,
}

impl PackageCacheReporterInner {
    fn on_validate_start(&mut self, entry_idx: usize) {
        self.packages[entry_idx].validate_started = true;
        self.rerender();
    }

    fn on_validate_complete(&mut self, entry_idx: usize) {
        self.packages[entry_idx].validate_completed = true;
        self.rerender();
    }

    fn on_download_start(&mut self, entry_idx: usize) {
        self.packages[entry_idx].download_started = true;
        self.rerender();
    }

    fn on_download_progress(&mut self, entry_idx: usize, progress: u64, total: Option<u64>) {
        let entry = &mut self.packages[entry_idx];
        entry.download_progress = Some(progress);
        entry.size = entry.size.or(total);
        self.rerender();
    }

    fn on_download_completed(&mut self, entry_idx: usize) {
        self.packages[entry_idx].download_completed = true;
        self.rerender();
    }

    fn style(&mut self, props: ProgressStyleProperties) -> ProgressStyle {
        if let Some(style) = self.style_cache.get(&props) {
            return style.clone();
        }

        let style = self.build_style(&props);
        self.style_cache.insert(props, style.clone());
        style
    }

    fn build_style(&self, props: &ProgressStyleProperties) -> ProgressStyle {
        let mut result = self.prefix.to_string();

        // Add a spinner
        match props.status {
            ProgressStatus::Paused => result.push_str("{spinner:.dim} "),
            ProgressStatus::Active => result.push_str("{spinner:.green} "),
            ProgressStatus::Finished => result.push_str(&format!(
                "{} ",
                console::style(console::Emoji("✔", " ")).green()
            )),
        }

        // Add a prefix
        result.push_str("{prefix:20!} ");

        // Add progress indicator
        if props.status != ProgressStatus::Finished {
            if props.status == ProgressStatus::Active {
                result.push_str("[{elapsed_precise}] [{bar:20!.bright.yellow/dim.white}] ");
            } else {
                result.push_str("[{elapsed_precise}] [{bar:20!.dim.yellow/dim.white}] ");
            }
            match props.progress_type {
                ProgressType::Generic => {
                    // Don't show position and total, because these are visible
                    // through text anyway.
                }
                ProgressType::Bytes => result.push_str("{bytes:>8} / {total_bytes:<8} "),
            }
        } else {
            result.push_str(&format!("{}", console::style("done").green()))
        }

        // Add message
        result.push_str("{msg:.dim}");

        indicatif::ProgressStyle::with_template(&result)
            .expect("failed to create default style")
            .progress_chars(&self.progress_chars)
    }

    fn rerender(&mut self) {
        self.rerender_validation();
        self.rerender_download();
    }

    fn rerender_validation(&mut self) {
        // Find activate validation entries
        let validating_packages: Vec<_> = self
            .packages
            .iter()
            .filter(|e| e.validate_started)
            .collect();
        if validating_packages.is_empty() && self.validation_pb.is_none() {
            // If there are no packages to validate, we don't need to do anything.
            return;
        }

        // The total length of the progress bar
        let total = validating_packages
            .iter()
            .map(|r| r.size.unwrap_or(1))
            .sum::<u64>();

        // The number of packages that finish validation.
        let position: u64 = validating_packages
            .iter()
            .filter(|r| r.validate_completed)
            .map(|r| r.size.unwrap_or(1))
            .sum();

        // True if all packages have completed validation or have started downloading.
        let completed = self
            .packages
            .iter()
            .all(|r| r.validate_completed || r.download_started);
        let pending = self
            .packages
            .iter()
            .all(|r| !r.validate_started || r.validate_completed);

        let remaining: Vec<_> = validating_packages
            .into_iter()
            .filter(|r| !r.validate_completed)
            .collect();
        let msg = if remaining.is_empty() {
            String::new()
        } else {
            format_progress_message(remaining)
        };

        let style = self.style(ProgressStyleProperties {
            status: if completed {
                ProgressStatus::Finished
            } else if pending {
                ProgressStatus::Paused
            } else {
                ProgressStatus::Active
            },
            progress_type: ProgressType::Generic,
        });

        // Make sure there is a progress bar.
        match &self.validation_pb {
            Some(pb) => {
                pb.set_style(style);
                pb.set_length(total);
                pb.set_position(position);
                pb.set_message(msg)
            }
            None => {
                let pb = ProgressBar::hidden()
                    .with_position(position)
                    .with_prefix("validate cache")
                    .with_style(style)
                    .with_finish(ProgressFinish::AndLeave)
                    .with_message(msg);

                pb.set_length(total);
                pb.enable_steady_tick(Duration::from_millis(100));

                let pb = if let Some(download_pb) = &self.download_pb {
                    self.multi_progress.insert_before(download_pb, pb)
                } else {
                    self.placement
                        .insert(self.multi_progress.clone(), ProgressBar::new_spinner())
                };

                self.validation_pb = Some(pb);
            }
        };
    }

    fn rerender_download(&mut self) {
        // Find activate validation entries
        let downloading_packages: Vec<_> = self
            .packages
            .iter()
            .filter(|e| e.download_started)
            .collect();
        if downloading_packages.is_empty() && self.download_pb.is_none() {
            // If there are no packages to validate, we don't need to do anything.
            return;
        }

        // The total length of the progress bar
        let total = downloading_packages
            .iter()
            .map(|r| r.size.unwrap_or(1))
            .sum::<u64>();

        // The total number of bytes downloaded
        let position: u64 = downloading_packages
            .iter()
            .filter_map(|r| r.download_progress)
            .sum();

        // True if all packages have completed validation and have started downloading.
        let completed = self.packages.iter().all(|r| {
            r.download_started
                && r.download_completed
                && (!r.validate_started || r.validate_completed)
        });
        let pending = self
            .packages
            .iter()
            .all(|r| !r.download_started || r.download_completed);

        let remaining: Vec<_> = downloading_packages
            .into_iter()
            .filter(|r| !r.download_completed)
            .collect();
        let msg = if remaining.is_empty() {
            String::new()
        } else {
            format_progress_message(remaining)
        };

        let style = self.style(ProgressStyleProperties {
            status: if completed {
                ProgressStatus::Finished
            } else if pending {
                ProgressStatus::Paused
            } else {
                ProgressStatus::Active
            },
            progress_type: ProgressType::Bytes,
        });

        // Make sure there is a progress bar.
        match &self.download_pb {
            Some(pb) => {
                pb.set_style(style);
                pb.set_length(total);
                pb.set_position(position);
                pb.set_message(msg);
            }
            None => {
                let pb = ProgressBar::hidden()
                    .with_position(position)
                    .with_prefix("download & extract")
                    .with_style(style)
                    .with_message(msg)
                    .with_finish(ProgressFinish::AndLeave);

                pb.set_length(total);
                pb.enable_steady_tick(Duration::from_millis(100));

                let pb = if let Some(validation_pb) = &self.validation_pb {
                    self.multi_progress.insert_after(validation_pb, pb)
                } else {
                    self.placement
                        .insert(self.multi_progress.clone(), ProgressBar::new_spinner())
                };

                self.download_pb = Some(pb);
            }
        };
    }
}

impl Drop for PackageCacheReporterInner {
    fn drop(&mut self) {
        if let Some(pb) = self.validation_pb.take() {
            pb.finish_and_clear();
        }
        if let Some(pb) = self.download_pb.take() {
            pb.finish_and_clear();
        }
    }
}

fn format_progress_message(remaining: Vec<&ProgressEntry>) -> String {
    let mut msg = String::new();
    let largest_package = remaining.iter().max_by_key(|e| e.size.unwrap_or(0));
    if let Some(e) = largest_package {
        msg.push_str(&e.name);
    }

    let count = remaining.len();
    if count > 1 {
        msg.push_str(&format!(" (+{})", count - 1));
    }

    msg
}

struct ProgressEntry {
    /// The name of the package
    name: String,

    /// The size of the package in bytes or `None` if we don't know the size.
    size: Option<u64>,

    validate_started: bool,
    validate_completed: bool,

    download_started: bool,
    download_progress: Option<u64>,

    download_completed: bool,
}

pub struct PackageCacheReporterEntry {
    inner: Arc<Mutex<PackageCacheReporterInner>>,
    entry_idx: usize,
}

impl CacheReporter for PackageCacheReporterEntry {
    fn on_validate_start(&self) -> usize {
        self.inner.lock().unwrap().on_validate_start(self.entry_idx);
        self.entry_idx
    }

    fn on_validate_complete(&self, index: usize) {
        debug_assert!(index == self.entry_idx);
        self.inner
            .lock()
            .unwrap()
            .on_validate_complete(self.entry_idx);
    }

    fn on_download_start(&self) -> usize {
        self.inner.lock().unwrap().on_download_start(self.entry_idx);
        self.entry_idx
    }

    fn on_download_progress(&self, index: usize, progress: u64, total: Option<u64>) {
        debug_assert!(index == self.entry_idx);
        self.inner
            .lock()
            .unwrap()
            .on_download_progress(self.entry_idx, progress, total);
    }

    fn on_download_completed(&self, index: usize) {
        debug_assert!(index == self.entry_idx);
        self.inner
            .lock()
            .unwrap()
            .on_download_completed(self.entry_idx);
    }
}
