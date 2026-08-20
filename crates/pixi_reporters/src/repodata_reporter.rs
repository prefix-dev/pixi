use std::{
    collections::HashSet,
    fmt::Write,
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use indexmap::IndexMap;
use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle, style::ProgressTracker};
use jiff::{
    Span, Timestamp,
    fmt::friendly::{Designator, Spacing, SpanPrinter},
};
use parking_lot::{Mutex, RwLock};
use pixi_progress::ProgressBarPlacement;
use rattler_conda_types::{ChannelNoticeLevel, ChannelUrl};
use rattler_repodata_gateway::{
    ChannelNoticeResult, ChannelRelationsWarning, DownloadReporter, GatewayWarning,
    UnsupportedRepodataRevision,
};
use url::Url;

#[derive(Clone)]
pub struct RepodataReporter {
    inner: Arc<RwLock<RepodataReporterInner>>,
}

impl rattler_repodata_gateway::Reporter for RepodataReporter {
    fn download_reporter(&self) -> Option<&dyn DownloadReporter> {
        Some(self)
    }

    fn on_unsupported_repodata_revision(&self, message: &UnsupportedRepodataRevision) {
        let mut inner = self.inner.write();
        inner.on_unsupported_repodata_revision(message);
    }

    fn on_gateway_warning(&self, warning: &GatewayWarning) {
        match warning {
            // CEP-42 makes the user's channel order authoritative, so an
            // overridden relation is the specified outcome, not a problem.
            GatewayWarning::ChannelRelations(ChannelRelationsWarning::UserOrderConflict {
                ..
            }) => tracing::debug!("{warning}"),
            _ => tracing::warn!("{warning}"),
        }
    }

    fn on_channel_notice(&self, notice: &ChannelNoticeResult) {
        queue_channel_notice(notice);
    }
}

impl RepodataReporter {
    pub fn clear(&self) {
        self.inner.write().clear();
    }
}

static CHANNEL_NOTICES: LazyLock<Mutex<IndexMap<(ChannelUrl, String), ChannelNoticeResult>>> =
    LazyLock::new(|| Mutex::new(IndexMap::new()));
const VIEWED_CHANNEL_NOTICES_FILE: &str = "viewed-notices-v1";

/// Queue a CEP-6 channel notice for display at the end of the CLI command.
pub fn queue_channel_notice(notice: &ChannelNoticeResult) {
    CHANNEL_NOTICES
        .lock()
        .entry((notice.channel.clone(), notice.notice.id.clone()))
        .or_insert_with(|| notice.clone());
}

/// Display all queued, previously unseen channel notices in arrival order.
pub fn display_channel_notices() {
    let notices = CHANNEL_NOTICES.lock().drain(..).collect::<Vec<_>>();
    let mut viewed = read_viewed_channel_notice_ids();
    let mut newly_viewed = Vec::new();

    for (_, notice) in notices {
        if viewed.insert(notice.notice.id.clone()) {
            pixi_progress::println!(
                "{}",
                format_channel_notice(&notice, Timestamp::now(), console::colors_enabled_stderr())
            );
            newly_viewed.push(notice.notice.id);
        }
    }

    if !newly_viewed.is_empty()
        && let Err(err) = write_viewed_channel_notice_ids(&viewed)
    {
        tracing::debug!("failed to cache viewed channel notices: {err}");
    }
}

fn viewed_channel_notices_path() -> Option<PathBuf> {
    pixi_config::get_cache_dir().ok().map(|cache_dir| {
        cache_dir
            .join(pixi_consts::consts::CHANNEL_NOTICES_CACHE_DIR)
            .join(VIEWED_CHANNEL_NOTICES_FILE)
    })
}

fn read_viewed_channel_notice_ids() -> HashSet<String> {
    viewed_channel_notices_path()
        .map(|path| read_viewed_channel_notice_ids_from(&path))
        .unwrap_or_default()
}

fn read_viewed_channel_notice_ids_from(path: &std::path::Path) -> HashSet<String> {
    fs_err::read_to_string(path)
        .map(|contents| {
            contents
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn write_viewed_channel_notice_ids(viewed: &HashSet<String>) -> std::io::Result<()> {
    let Some(path) = viewed_channel_notices_path() else {
        return Ok(());
    };
    write_viewed_channel_notice_ids_to(&path, viewed)
}

fn write_viewed_channel_notice_ids_to(
    path: &std::path::Path,
    viewed: &HashSet<String>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs_err::create_dir_all(parent)?;
    }
    let mut ids = viewed.iter().map(String::as_str).collect::<Vec<_>>();
    ids.sort_unstable();
    fs_err::write(path, ids.join("\n"))
}

fn format_channel_notice(notice: &ChannelNoticeResult, now: Timestamp, color: bool) -> String {
    let (symbol, label, ansi) = match notice.notice.level {
        ChannelNoticeLevel::Info => ("ℹ", "Info", "32"),
        ChannelNoticeLevel::Warning => ("⚠", "Warning", "33"),
        ChannelNoticeLevel::Critical => ("✖", "Critical", "31"),
    };
    let severity = if color {
        format!("\x1b[1;{ansi}m{symbol} {label}\x1b[0m")
    } else {
        format!("{symbol} {label}")
    };

    let mut channel = notice.channel.as_ref().clone();
    if !channel.username().is_empty() {
        let _ = channel.set_username("***");
    }
    let _ = channel.set_password(None);

    let mut rendered = format!("\n  ╭─ {severity} channel notice\n");
    for line in notice.notice.message.split('\n') {
        push_wrapped_notice_line(&mut rendered, line);
    }
    rendered.push_str("  │\n  │ Channel  ");
    rendered.push_str(channel.as_str().trim_end_matches('/'));
    rendered.push('\n');
    if let Some(created_at) = notice.notice.created_at {
        rendered.push_str("  │ Added    ");
        rendered.push_str(&format_relative_time(created_at, now));
        rendered.push('\n');
    }
    if let Some(expires_at) = notice.notice.expires_at {
        rendered.push_str("  │ Expires  ");
        rendered.push_str(&format_relative_time(expires_at, now));
        rendered.push('\n');
    }
    rendered.push_str("  ╰─\n");
    rendered
}

fn format_relative_time(timestamp: Timestamp, now: Timestamp) -> String {
    let seconds = timestamp.as_second().saturating_sub(now.as_second());
    let absolute = seconds.unsigned_abs();
    if absolute < 60 {
        return if seconds > 0 {
            "in less than a minute".to_owned()
        } else {
            "just now".to_owned()
        };
    }

    let amount = if absolute < 60 * 60 {
        Span::new().minutes((absolute / 60) as i64)
    } else if absolute < 24 * 60 * 60 {
        Span::new().hours((absolute / (60 * 60)) as i64)
    } else {
        Span::new().days((absolute / (24 * 60 * 60)) as i64)
    };
    let amount = if seconds < 0 { -amount } else { amount };
    let formatted = SpanPrinter::new()
        .designator(Designator::Verbose)
        .spacing(Spacing::BetweenUnitsAndDesignators)
        .span_to_string(&amount);

    if seconds > 0 {
        format!("in {formatted}")
    } else {
        formatted
    }
}

fn push_wrapped_notice_line(rendered: &mut String, line: &str) {
    const MESSAGE_WIDTH: usize = 88;

    let mut remainder = line;
    loop {
        let boundary = remainder.char_indices().nth(MESSAGE_WIDTH).map(|(i, _)| i);
        let Some(boundary) = boundary else {
            rendered.push_str("  │ ");
            rendered.push_str(remainder);
            rendered.push('\n');
            return;
        };
        let split = remainder[..boundary]
            .rfind(char::is_whitespace)
            .filter(|index| *index > 0)
            .unwrap_or(boundary);
        rendered.push_str("  │ ");
        rendered.push_str(remainder[..split].trim_end());
        rendered.push('\n');
        remainder = remainder[split..].trim_start();
    }
}

struct RepodataReporterInner {
    pb: ProgressBar,
    title: Option<String>,
    downloads: Arc<RwLock<Vec<TrackedDownload>>>,
    unsupported_revision_warnings: HashSet<String>,
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
                unsupported_revision_warnings: HashSet::new(),
            })),
        }
    }
}

impl RepodataReporterInner {
    pub fn clear(&mut self) {
        self.pb.finish_and_clear();
        self.downloads.write().clear();
    }

    fn on_unsupported_repodata_revision(&mut self, message: &UnsupportedRepodataRevision) {
        let message = message.to_string();
        if self.unsupported_revision_warnings.insert(message.clone()) {
            pixi_progress::println!(
                "{}",
                console::style(format!(
                    "warning: {message}. Update pixi to read those records."
                ))
                .yellow()
            );
        }
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

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use rattler_conda_types::{ChannelNotice, ChannelNoticeLevel};
    use rattler_repodata_gateway::ChannelNoticeResult;

    use super::{
        format_channel_notice, format_relative_time, read_viewed_channel_notice_ids_from,
        write_viewed_channel_notice_ids_to,
    };

    fn notice(level: ChannelNoticeLevel) -> ChannelNoticeResult {
        ChannelNoticeResult {
            channel: url::Url::parse("https://token:secret@example.com/channel/")
                .unwrap()
                .into(),
            notice: ChannelNotice {
                id: "notice-id".to_owned(),
                message: "A channel notice".to_owned(),
                level,
                created_at: Some("2026-08-17T10:30:00Z".parse().unwrap()),
                expires_at: Some("2026-08-30T10:30:00Z".parse().unwrap()),
                interval: None,
            },
        }
    }

    #[test]
    fn channel_notice_matches_microrattler_rendering() {
        let rendered = format_channel_notice(
            &notice(ChannelNoticeLevel::Warning),
            "2026-08-20T10:30:00Z".parse::<Timestamp>().unwrap(),
            false,
        );

        assert!(rendered.starts_with("\n  ╭─ ⚠ Warning channel notice\n"));
        assert!(rendered.contains("  │ A channel notice\n  │\n"));
        assert!(rendered.contains("  │ Channel  https://***@example.com/channel\n"));
        assert!(rendered.contains("  │ Added    3 days ago\n"));
        assert!(rendered.contains("  │ Expires  in 10 days\n"));
        assert!(rendered.ends_with("  ╰─\n"));
        assert!(!rendered.contains("token"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn channel_notice_severity_colors_match_microrattler() {
        let now = Timestamp::now();
        let rendered = [
            (ChannelNoticeLevel::Info, "\x1b[1;32mℹ Info\x1b[0m"),
            (ChannelNoticeLevel::Warning, "\x1b[1;33m⚠ Warning\x1b[0m"),
            (ChannelNoticeLevel::Critical, "\x1b[1;31m✖ Critical\x1b[0m"),
        ];

        for (level, expected) in rendered {
            assert!(format_channel_notice(&notice(level), now, true).contains(expected));
        }
    }

    #[test]
    fn relative_times_use_jiff_friendly_formatting() {
        let now = "2026-08-20T10:30:00Z".parse::<Timestamp>().unwrap();

        assert_eq!(
            format_relative_time("2026-08-20T09:30:00Z".parse().unwrap(), now),
            "1 hour ago"
        );
        assert_eq!(
            format_relative_time("2026-08-20T10:32:00Z".parse().unwrap(), now),
            "in 2 minutes"
        );
    }

    #[test]
    fn viewed_channel_notice_ids_are_persisted() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("notices/viewed-notices-v1");
        let viewed = ["second", "first"].into_iter().map(str::to_owned).collect();

        write_viewed_channel_notice_ids_to(&path, &viewed).unwrap();

        assert_eq!(fs_err::read_to_string(&path).unwrap(), "first\nsecond");
        assert_eq!(read_viewed_channel_notice_ids_from(&path), viewed);
    }
}
