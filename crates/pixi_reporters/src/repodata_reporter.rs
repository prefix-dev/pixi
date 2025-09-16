use std::{
    fmt::Write,
    sync::Arc,
    time::{Duration, Instant},
};

use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle, style::ProgressTracker};
use parking_lot::RwLock;
use pixi_progress::ProgressBarPlacement;
use rattler_repodata_gateway::{DownloadReporter, JLAPReporter};
use url::Url;

#[derive(Clone)]
pub struct RepodataReporter {
    inner: Arc<RwLock<RepodataReporterInner>>,
}

impl rattler_repodata_gateway::Reporter for RepodataReporter {
    fn download_reporter(&self) -> Option<&dyn DownloadReporter> {
        Some(self)
    }

    fn jlap_reporter(&self) -> Option<&dyn JLAPReporter> {
        // TODO: Implement JLAPReporter for RepodataReporter in the future
        None
    }
}

impl RepodataReporter {
    pub fn clear(&self) {
        self.inner.write().clear();
    }
}

struct RepodataReporterInner {
    pb: ProgressBar,
    title: Option<String>,
    downloads: Arc<RwLock<Vec<TrackedDownload>>>,
}

struct TrackedDownload {
    started: Instant,
    finished: Option<Instant>,
    total_bytes: Option<usize>,
    bytes_downloaded: usize,
}

impl RepodataReporter {
    pub fn new(
        multi_progress: MultiProgress,
        progress_bar_placement: ProgressBarPlacement,
        title: String,
    ) -> Self {
        let pb = progress_bar_placement.insert(multi_progress, ProgressBar::hidden());
        Self {
            inner: Arc::new(RwLock::new(RepodataReporterInner {
                pb,
                title: Some(title),
                downloads: Arc::new(RwLock::new(Vec::new())),
            })),
        }
    }
}

impl RepodataReporterInner {
    pub fn clear(&mut self) {
        self.pb.finish_and_clear();
        self.downloads.write().clear();
    }

    pub fn update(&mut self) {
        let downloads = self.downloads.read();
        if !downloads.iter().any(|d| d.bytes_downloaded > 0) {
            // Dont do anything if no downloads have been started.
            return;
        }

        let bytes_downloaded = downloads.iter().map(|d| d.bytes_downloaded).sum::<usize>();
        let total_bytes = downloads
            .iter()
            .map(|d| d.total_bytes.unwrap_or(d.bytes_downloaded))
            .sum::<usize>();
        let pending_downloads = downloads
            .iter()
            .any(|d| d.finished.is_none() && d.bytes_downloaded > 0);

        // Set the style of the progress bar.
        let verbose = tracing::event_enabled!(tracing::Level::INFO);
        self.pb.set_style(
            ProgressStyle::with_template(&format!(
                "{{spinner:.{spinner}}} {{prefix:20!}} [{{bar:20!.bright.yellow/dim.white}}] {verbose}{speed}",
                spinner = if pending_downloads { "green" } else { "dim" },
                verbose = if verbose { format!("{{bytes:>2.dim}}{slash}{{total_bytes:>2.dim}} ", slash = console::style("/").dim()) } else { String::new() },
                speed = if pending_downloads { format!("{at} {{speed:.dim}}", at = console::style("@").dim()) } else { String::new() }
            ))
            .expect("failed to create progress bar style")
            .tick_chars(pixi_progress::style::tick_chars(pending_downloads))
            .progress_chars(pixi_progress::style::progress_chars(pending_downloads))
            .with_key(
                "speed",
                DurationTracker::new(self.downloads.clone()),
            )
        );

        // Set the title of the progress bar if it is was missing
        if let Some(title) = self.title.take() {
            self.pb.set_prefix(title);
            self.pb.enable_steady_tick(Duration::from_millis(100));
        }
        self.pb.set_length(total_bytes as u64);
        self.pb.set_position(bytes_downloaded as u64);
    }

    fn on_download_start(&mut self, _url: &Url) -> usize {
        let mut downloads = self.downloads.write();
        let id = downloads.len();
        downloads.push(TrackedDownload {
            started: Instant::now(),
            finished: None,
            total_bytes: None,
            bytes_downloaded: 0,
        });
        drop(downloads);
        self.update();
        id
    }

    fn on_download_progress(
        &mut self,
        _url: &Url,
        index: usize,
        bytes_downloaded: usize,
        total_bytes: Option<usize>,
    ) {
        let mut downloads = self.downloads.write();
        let dwnld = &mut downloads[index];
        if let Some(total_bytes) = total_bytes {
            dwnld.total_bytes.get_or_insert(total_bytes);
        }
        dwnld.bytes_downloaded = bytes_downloaded;
        drop(downloads);
        self.update();
    }

    fn on_download_complete(&mut self, _url: &Url, index: usize) {
        let mut downloads = self.downloads.write();
        let dwnld = &mut downloads[index];
        dwnld.finished = Some(Instant::now());
        if let Some(total) = dwnld.total_bytes {
            dwnld.bytes_downloaded = dwnld.bytes_downloaded.max(total);
        }
        drop(downloads);
        self.update();
    }
}

/// Compute the total active time of all downloads.
///
/// This is useful for calculating the average download speed in a situation
/// where there could also not be a download active for a period of time.
///
/// The function calculates the total active download time from a slice of
/// `TrackedDownload` items, considering their start and finish times, and
/// returns the result as a `Duration`.
fn total_duration(items: &[TrackedDownload], now: Instant) -> Duration {
    let mut intervals: Vec<(Instant, Instant)> = items
        .iter()
        .filter(|d| d.bytes_downloaded > 0)
        .map(|item| (item.started, item.finished.unwrap_or(now)))
        .collect();

    // Sort intervals by start time
    intervals.sort_by_key(|(start, _)| *start);

    let mut total = Duration::ZERO;
    let mut current: Option<(Instant, Instant)> = None;

    for (start, end) in intervals {
        if let Some((cur_start, cur_end)) = current {
            if start <= cur_end {
                current = Some((cur_start, cur_end.max(end)));
            } else {
                total += cur_end.duration_since(cur_start);
                current = Some((start, end));
            }
        } else {
            current = Some((start, end));
        }
    }

    if let Some((cur_start, cur_end)) = current {
        total += cur_end.duration_since(cur_start);
    }

    total
}

/// This is a custom progress tracker that calculates the average download speed
/// while taking into account the total active time of all downloads.
#[derive(Clone)]
struct DurationTracker {
    inner: Arc<RwLock<Vec<TrackedDownload>>>,
    duration: Duration,
    len: u64,
}

impl DurationTracker {
    pub fn new(inner: Arc<RwLock<Vec<TrackedDownload>>>) -> Self {
        Self {
            inner,
            duration: Duration::ZERO,
            len: 0,
        }
    }
}

impl ProgressTracker for DurationTracker {
    fn clone_box(&self) -> Box<dyn ProgressTracker> {
        Box::new(self.clone())
    }

    fn tick(&mut self, state: &ProgressState, now: std::time::Instant) {
        let inner = self.inner.read();
        self.duration = total_duration(&inner, now);
        self.len = state.len().unwrap_or(0);
    }

    fn reset(&mut self, _state: &ProgressState, _now: std::time::Instant) {}

    fn write(&self, _state: &ProgressState, w: &mut dyn Write) {
        let total_secs = self.duration.as_secs_f64();
        if self.len == 0 || total_secs <= 0.0 {
            write!(w, "0B/s").unwrap();
        } else {
            let bytes_per_sec = self.len as f64 / total_secs;
            write!(
                w,
                "{bytes_per_sec}/s",
                bytes_per_sec = human_bytes::human_bytes(bytes_per_sec)
            )
            .unwrap();
        }
    }
}

impl DownloadReporter for RepodataReporter {
    fn on_download_complete(&self, url: &Url, index: usize) {
        let mut inner = self.inner.write();
        inner.on_download_complete(url, index);
    }

    fn on_download_progress(
        &self,
        url: &Url,
        index: usize,
        bytes_downloaded: usize,
        total_bytes: Option<usize>,
    ) {
        let mut inner = self.inner.write();
        inner.on_download_progress(url, index, bytes_downloaded, total_bytes);
    }

    fn on_download_start(&self, url: &Url) -> usize {
        let mut inner = self.inner.write();
        inner.on_download_start(url)
    }
}
