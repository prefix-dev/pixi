use std::{
    collections::{HashMap, HashSet},
    fmt::Write,
    path::{Path, PathBuf},
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
use rattler_conda_types::{ChannelNotice, ChannelNoticeLevel, ChannelUrl};
use rattler_redaction::{DEFAULT_REDACTION_STR, Redact};
use rattler_repodata_gateway::{
    ChannelNoticeResult, ChannelRelationsWarning, DownloadReporter, GatewayWarning,
    UnsupportedRepodataRevision,
};
use serde::{Deserialize, Serialize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
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

/// The file recording which channel notices have already been displayed.
///
/// `v2` retires a newline-separated format that could not represent a notice id
/// containing a newline and did not record which channel a notice came from.
const VIEWED_CHANNEL_NOTICES_FILE: &str = "viewed-notices-v2";
const VIEWED_CHANNEL_NOTICES_FILE_V1: &str = "viewed-notices-v1";

/// How long a notice stays recorded as viewed when its channel declares no
/// expiry. Without a bound the file would grow for as long as pixi is installed.
const VIEWED_NOTICE_RETENTION: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// The widest a notice body may render, in terminal columns.
const MAX_MESSAGE_WIDTH: usize = 88;

/// How much of a single notice is displayed. Any channel can publish a notice
/// and the gateway accepts a `notices.json` of up to a megabyte, so an
/// unbounded message would let a channel flood the terminal.
const MAX_MESSAGE_LINES: usize = 40;
const MAX_MESSAGE_CHARS: usize = 2000;

/// Query parameters channels use to carry credentials.
const SECRET_QUERY_PARAMS: &[&str] = &[
    "token",
    "access_token",
    "api_key",
    "password",
    "sig",
    "signature",
];

/// A notice that has already been shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ViewedNotice {
    /// The channel that published the notice.
    channel: String,
    /// The notice's CEP-6 id, unique only within its channel.
    id: String,
    /// When the notice was last displayed.
    seen_at: Timestamp,
    /// The expiry the channel declared, used to prune this entry later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<Timestamp>,
}

/// Viewed notices keyed by channel and id.
///
/// The channel is part of the key because CEP-6 ids are only unique within one
/// channel's `notices.json`; ids like `1` or `security-1` are common.
type ViewedNotices = HashMap<(String, String), ViewedNotice>;

/// Queue a CEP-6 channel notice for display at the end of the CLI command.
pub fn queue_channel_notice(notice: &ChannelNoticeResult) {
    CHANNEL_NOTICES
        .lock()
        .entry((notice.channel.clone(), notice.notice.id.clone()))
        .or_insert_with(|| notice.clone());
}

/// Display all queued channel notices that the user has not already seen.
pub fn display_channel_notices() {
    let notices = CHANNEL_NOTICES.lock().drain(..).collect::<Vec<_>>();
    if notices.is_empty() {
        return;
    }

    let now = Timestamp::now();
    let mut viewed = read_viewed_channel_notices();
    let width = notice_message_width();
    let color = console::colors_enabled_stderr();
    let mut displayed_any = false;

    for (_, notice) in notices {
        let key = (
            notice.channel.as_ref().as_str().to_owned(),
            notice.notice.id.clone(),
        );
        if !should_display(viewed.get(&key), &notice.notice, now) {
            continue;
        }

        pixi_progress::println!("{}", format_channel_notice(&notice, now, width, color));
        viewed.insert(
            key.clone(),
            ViewedNotice {
                channel: key.0,
                id: key.1,
                seen_at: now,
                expires_at: notice.notice.expires_at,
            },
        );
        displayed_any = true;
    }

    if !displayed_any {
        return;
    }

    if let Err(err) = write_viewed_channel_notices(&viewed, now) {
        // Worth a warning rather than a debug line: when this keeps failing the
        // same notice reappears on every command, and the cause is invisible
        // otherwise.
        tracing::warn!("failed to record viewed channel notices: {err}");
    }
}

/// Whether a notice should be shown, given what is already recorded for it.
fn should_display(previous: Option<&ViewedNotice>, notice: &ChannelNotice, now: Timestamp) -> bool {
    let Some(previous) = previous else {
        return true;
    };
    // CEP-6 lets a channel ask for a notice to be repeated every `interval`
    // seconds. A notice without one is shown exactly once.
    let Some(interval) = notice.interval else {
        return false;
    };
    let elapsed = now.as_second().saturating_sub(previous.seen_at.as_second());
    elapsed >= i64::try_from(interval).unwrap_or(i64::MAX)
}

/// Forget notices that have expired or have not been seen in a long time.
fn prune_viewed_channel_notices(viewed: &mut ViewedNotices, now: Timestamp) {
    let retention = i64::try_from(VIEWED_NOTICE_RETENTION.as_secs()).unwrap_or(i64::MAX);
    viewed.retain(|_, entry| match entry.expires_at {
        Some(expires_at) => expires_at > now,
        None => now.as_second().saturating_sub(entry.seen_at.as_second()) < retention,
    });
}

/// The directory holding pixi's channel notice state.
///
/// `pixi clean cache --notices` removes this directory, so both it and
/// `viewed_channel_notices_path` resolve it here to stay in agreement.
pub fn channel_notices_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(pixi_consts::consts::CHANNEL_NOTICES_CACHE_DIR)
}

fn viewed_channel_notices_path() -> Option<PathBuf> {
    pixi_config::get_cache_dir()
        .ok()
        .map(|cache_dir| channel_notices_cache_dir(&cache_dir).join(VIEWED_CHANNEL_NOTICES_FILE))
}

fn read_viewed_channel_notices() -> ViewedNotices {
    viewed_channel_notices_path()
        .map(|path| read_viewed_channel_notices_from(&path))
        .unwrap_or_default()
}

fn read_viewed_channel_notices_from(path: &Path) -> ViewedNotices {
    let Ok(contents) = fs_err::read_to_string(path) else {
        return ViewedNotices::default();
    };
    match serde_json::from_str::<Vec<ViewedNotice>>(&contents) {
        Ok(entries) => entries
            .into_iter()
            .map(|entry| ((entry.channel.clone(), entry.id.clone()), entry))
            .collect(),
        Err(err) => {
            // Starting over costs a repeated notice; treating a damaged file as
            // authoritative would suppress notices the user has never seen.
            tracing::debug!(
                "ignoring unreadable viewed channel notices at {}: {err}",
                path.display()
            );
            ViewedNotices::default()
        }
    }
}

fn write_viewed_channel_notices(viewed: &ViewedNotices, now: Timestamp) -> std::io::Result<()> {
    let Some(path) = viewed_channel_notices_path() else {
        return Err(std::io::Error::other(
            "could not determine the pixi cache directory",
        ));
    };
    write_viewed_channel_notices_to(&path, viewed, now)
}

fn write_viewed_channel_notices_to(
    path: &Path,
    viewed: &ViewedNotices,
    now: Timestamp,
) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::other(
            "viewed channel notices path has no parent directory",
        ));
    };
    fs_err::create_dir_all(parent)?;

    // Merge with whatever another pixi process recorded since this one read the
    // file, so parallel commands do not drop each other's entries. The rename
    // below is atomic, so a reader never observes a partial file. A write that
    // interleaves with another process's rename can still lose an entry, which
    // costs a repeated notice rather than a suppressed one.
    let mut merged = read_viewed_channel_notices_from(path);
    for (key, entry) in viewed {
        merged
            .entry(key.clone())
            .and_modify(|existing| {
                if entry.seen_at > existing.seen_at {
                    *existing = entry.clone();
                }
            })
            .or_insert_with(|| entry.clone());
    }
    prune_viewed_channel_notices(&mut merged, now);

    let mut entries = merged.into_values().collect::<Vec<_>>();
    entries.sort_by(|a, b| (&a.channel, &a.id).cmp(&(&b.channel, &b.id)));
    let serialized = serde_json::to_vec_pretty(&entries).map_err(std::io::Error::other)?;

    let temp = parent.join(format!(
        "{VIEWED_CHANNEL_NOTICES_FILE}.{}.tmp",
        std::process::id()
    ));
    fs_err::write(&temp, serialized)?;
    if let Err(err) = fs_err::rename(&temp, path) {
        let _ = fs_err::remove_file(&temp);
        return Err(err);
    }

    // Retire the format this file replaces.
    let _ = fs_err::remove_file(parent.join(VIEWED_CHANNEL_NOTICES_FILE_V1));
    Ok(())
}

/// Removes channel credentials from a URL before it is displayed.
///
/// [`Redact`] covers the password and the `/t/<token>/` path segment that
/// prefix.dev and anaconda.org use. The username and the query parameters that
/// carry tokens are handled here.
fn redact_channel_url(channel: &ChannelUrl) -> Url {
    let mut url = channel.as_ref().clone().redact();

    if !url.username().is_empty() {
        let _ = url.set_username(DEFAULT_REDACTION_STR);
    }

    if url.query().is_some() {
        let redacted = url
            .query_pairs()
            .map(|(key, value)| {
                let secret = SECRET_QUERY_PARAMS
                    .iter()
                    .any(|param| key.eq_ignore_ascii_case(param));
                let value = if secret {
                    DEFAULT_REDACTION_STR.to_owned()
                } else {
                    value.into_owned()
                };
                (key.into_owned(), value)
            })
            .collect::<Vec<_>>();
        url.query_pairs_mut().clear().extend_pairs(redacted);
    }

    url
}

/// Splits a notice message into displayable lines.
///
/// The message comes from a channel's `notices.json`, so it is remote input.
/// Escape sequences would let a channel repaint the terminal or hide text
/// behind a carriage return, and bidirectional controls would let it reorder
/// what the user reads, so neither survives here.
fn notice_message_lines(message: &str) -> Vec<String> {
    let normalized = message.replace("\r\n", "\n").replace('\r', "\n");

    let mut lines = Vec::new();
    let mut chars_used = 0;
    let mut truncated = false;

    for line in normalized.split('\n') {
        if lines.len() >= MAX_MESSAGE_LINES {
            truncated = true;
            break;
        }

        let mut sanitized = String::new();
        for ch in line.chars() {
            if ch == '\t' {
                sanitized.push_str("    ");
            } else if !is_terminal_control(ch) {
                sanitized.push(ch);
            }
        }

        if chars_used + sanitized.chars().count() > MAX_MESSAGE_CHARS {
            let remaining = MAX_MESSAGE_CHARS.saturating_sub(chars_used);
            sanitized = sanitized.chars().take(remaining).collect();
            truncated = true;
        }
        chars_used += sanitized.chars().count();

        let done = truncated;
        lines.push(sanitized);
        if done {
            break;
        }
    }

    if truncated {
        lines.push(String::new());
        lines.push("[notice truncated by pixi]".to_owned());
    }

    lines
}

/// Whether a character could take over the terminal or reorder the text.
fn is_terminal_control(ch: char) -> bool {
    ch.is_control()
        || matches!(ch,
            '\u{200E}' | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{2028}' | '\u{2029}'
            | '\u{FEFF}')
}

/// How wide a notice body may be, in terminal columns.
fn notice_message_width() -> usize {
    // Four columns go to the `  │ ` gutter.
    const GUTTER: usize = 4;
    const MIN_WIDTH: usize = 20;

    console::Term::stderr()
        .size_checked()
        .map(|(_, columns)| usize::from(columns).saturating_sub(GUTTER))
        .unwrap_or(MAX_MESSAGE_WIDTH)
        .clamp(MIN_WIDTH, MAX_MESSAGE_WIDTH)
}

fn format_channel_notice(
    notice: &ChannelNoticeResult,
    now: Timestamp,
    width: usize,
    color: bool,
) -> String {
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

    let channel = redact_channel_url(&notice.channel);

    let mut rendered = format!("\n  ╭─ {severity} channel notice\n");
    for line in notice_message_lines(&notice.notice.message) {
        push_wrapped_notice_line(&mut rendered, &line, width);
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

fn push_wrapped_notice_line(rendered: &mut String, line: &str, width: usize) {
    let mut remainder = line;
    loop {
        if remainder.width() <= width {
            rendered.push_str("  │ ");
            rendered.push_str(remainder);
            rendered.push('\n');
            return;
        }

        let boundary = width_boundary(remainder, width);
        let split = remainder[..boundary]
            .rfind(char::is_whitespace)
            .filter(|index| *index > 0)
            .unwrap_or(boundary);
        rendered.push_str("  │ ");
        rendered.push_str(remainder[..split].trim_end());
        rendered.push('\n');
        remainder = remainder[split..].trim_start();
        if remainder.is_empty() {
            return;
        }
    }
}

/// The byte index at which `line` first exceeds `width` display columns.
fn width_boundary(line: &str, width: usize) -> usize {
    let mut used = 0;
    for (index, ch) in line.char_indices() {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > width {
            // Always make progress, even for a character wider than the whole
            // available width.
            return if index == 0 { ch.len_utf8() } else { index };
        }
        used += ch_width;
    }
    line.len()
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
    use rattler_conda_types::{ChannelNotice, ChannelNoticeLevel, ChannelUrl};
    use rattler_repodata_gateway::ChannelNoticeResult;
    use unicode_width::UnicodeWidthStr;

    use super::{
        MAX_MESSAGE_LINES, ViewedNotice, ViewedNotices, format_channel_notice,
        format_relative_time, notice_message_lines, prune_viewed_channel_notices,
        push_wrapped_notice_line, read_viewed_channel_notices_from, redact_channel_url,
        should_display, write_viewed_channel_notices_to,
    };

    const WIDTH: usize = 88;

    fn at(timestamp: &str) -> Timestamp {
        timestamp.parse().expect("valid timestamp")
    }

    fn channel(url: &str) -> ChannelUrl {
        url::Url::parse(url).expect("valid url").into()
    }

    fn notice_on(channel_url: &str, id: &str, message: &str) -> ChannelNoticeResult {
        ChannelNoticeResult {
            channel: channel(channel_url),
            notice: ChannelNotice {
                id: id.to_owned(),
                message: message.to_owned(),
                level: ChannelNoticeLevel::Warning,
                created_at: Some(at("2026-08-17T10:30:00Z")),
                expires_at: Some(at("2026-08-30T10:30:00Z")),
                interval: None,
            },
        }
    }

    fn notice(level: ChannelNoticeLevel) -> ChannelNoticeResult {
        let mut notice = notice_on(
            "https://token:secret@example.com/channel/",
            "notice-id",
            "A channel notice",
        );
        notice.notice.level = level;
        notice
    }

    fn viewed(channel_url: &str, id: &str, seen_at: &str) -> ViewedNotice {
        ViewedNotice {
            channel: channel_url.to_owned(),
            id: id.to_owned(),
            seen_at: at(seen_at),
            expires_at: None,
        }
    }

    // -- Rendering ---------------------------------------------------------

    #[test]
    fn channel_notice_matches_microrattler_rendering() {
        let rendered = format_channel_notice(
            &notice(ChannelNoticeLevel::Warning),
            at("2026-08-20T10:30:00Z"),
            WIDTH,
            false,
        );

        assert!(rendered.starts_with("\n  ╭─ ⚠ Warning channel notice\n"));
        assert!(rendered.contains("  │ A channel notice\n  │\n"));
        assert!(rendered.contains("  │ Added    3 days ago\n"));
        assert!(rendered.contains("  │ Expires  in 10 days\n"));
        assert!(rendered.ends_with("  ╰─\n"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn channel_notice_severity_colors_match_microrattler() {
        let now = Timestamp::now();
        for (level, expected) in [
            (ChannelNoticeLevel::Info, "\x1b[1;32mℹ Info\x1b[0m"),
            (ChannelNoticeLevel::Warning, "\x1b[1;33m⚠ Warning\x1b[0m"),
            (ChannelNoticeLevel::Critical, "\x1b[1;31m✖ Critical\x1b[0m"),
        ] {
            assert!(format_channel_notice(&notice(level), now, WIDTH, true).contains(expected));
        }
    }

    #[test]
    fn relative_times_use_jiff_friendly_formatting() {
        let now = at("2026-08-20T10:30:00Z");

        assert_eq!(
            format_relative_time(at("2026-08-20T09:30:00Z"), now),
            "1 hour ago"
        );
        assert_eq!(
            format_relative_time(at("2026-08-20T10:32:00Z"), now),
            "in 2 minutes"
        );
    }

    // -- Credential redaction ----------------------------------------------

    #[test]
    fn credentials_are_redacted_wherever_a_channel_carries_them() {
        for (url, secret) in [
            ("https://user:hunter2@example.com/channel", "hunter2"),
            (
                "https://conda.anaconda.org/t/ac-12345-secret/my-channel",
                "ac-12345-secret",
            ),
            (
                "https://repo.prefix.dev/t/pfx_liveSECRET/private",
                "pfx_liveSECRET",
            ),
            ("https://example.com/c/?token=supersecret", "supersecret"),
            (
                "https://example.com/c/?access_token=supersecret",
                "supersecret",
            ),
        ] {
            let redacted = redact_channel_url(&channel(url)).to_string();
            assert!(
                !redacted.contains(secret),
                "secret survived redaction of {url}: {redacted}"
            );
        }
    }

    #[test]
    fn redaction_keeps_the_parts_that_identify_the_channel() {
        let redacted =
            redact_channel_url(&channel("https://conda.anaconda.org/t/tok/my-channel")).to_string();

        assert!(redacted.starts_with("https://conda.anaconda.org/t/"));
        assert!(redacted.contains("/my-channel"));
    }

    #[test]
    fn a_channel_url_without_credentials_is_left_alone() {
        let url = channel("https://conda.anaconda.org/conda-forge");
        assert_eq!(
            redact_channel_url(&url).to_string(),
            url.as_ref().to_string()
        );
    }

    // -- Terminal safety ---------------------------------------------------

    #[test]
    fn escape_sequences_from_a_channel_never_reach_the_terminal() {
        let hostile = "safe \u{1b}[2J\u{1b}[1;1H\u{1b}]0;pwned\u{7} text";
        let rendered = format_channel_notice(
            &notice_on("https://example.com/c", "id", hostile),
            Timestamp::now(),
            WIDTH,
            false,
        );

        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\u{7}'));
        assert!(rendered.contains("safe [2J[1;1H]0;pwned text"));
    }

    #[test]
    fn carriage_returns_become_line_breaks_instead_of_hiding_text() {
        let lines = notice_message_lines("before\rafter\r\nlast");
        assert_eq!(lines, vec!["before", "after", "last"]);
    }

    #[test]
    fn bidirectional_overrides_cannot_reorder_the_message() {
        let lines = notice_message_lines("safe \u{202E}gnisirprus\u{202C} text");
        assert_eq!(lines, vec!["safe gnisirprus text"]);
    }

    #[test]
    fn an_oversized_message_is_truncated() {
        let flood = (0..500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = notice_message_lines(&flood);

        assert!(
            lines.len() <= MAX_MESSAGE_LINES + 2,
            "{} lines",
            lines.len()
        );
        assert_eq!(
            lines.last().map(String::as_str),
            Some("[notice truncated by pixi]")
        );
    }

    // -- Wrapping ----------------------------------------------------------

    #[test]
    fn wrapping_respects_display_width_not_character_count() {
        for body in ["急急如律令".repeat(40), "🎉".repeat(80), "a".repeat(400)] {
            let mut rendered = String::new();
            push_wrapped_notice_line(&mut rendered, &body, WIDTH);

            for line in rendered.lines() {
                let body_width = line.strip_prefix("  │ ").unwrap_or(line).width();
                assert!(body_width <= WIDTH, "line is {body_width} columns: {line}");
            }
        }
    }

    #[test]
    fn wrapping_terminates_on_pathological_input() {
        let samples = [
            String::new(),
            " ".to_owned(),
            "\u{200B}".repeat(200),
            "a\u{0301}".repeat(200),
            "🎉".to_owned(),
            "word ".repeat(100),
        ];
        for sample in samples {
            let mut rendered = String::new();
            push_wrapped_notice_line(&mut rendered, &sample, WIDTH);
            assert!(rendered.starts_with("  │"));
        }
    }

    #[test]
    fn a_character_wider_than_the_line_still_makes_progress() {
        let mut rendered = String::new();
        push_wrapped_notice_line(&mut rendered, &"🎉".repeat(4), 1);
        assert_eq!(rendered.lines().count(), 4);
    }

    // -- Viewed-notice cache -----------------------------------------------

    #[test]
    fn viewed_notices_round_trip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("notices/viewed-notices-v2");
        let mut cache = ViewedNotices::default();
        cache.insert(
            ("https://example.com/c".to_owned(), "first".to_owned()),
            viewed("https://example.com/c", "first", "2026-08-20T10:30:00Z"),
        );

        write_viewed_channel_notices_to(&path, &cache, at("2026-08-20T10:30:00Z")).unwrap();

        assert_eq!(read_viewed_channel_notices_from(&path), cache);
    }

    /// CEP-6 ids are unique only within one channel, so two channels can both
    /// publish `1` and neither may hide the other.
    #[test]
    fn the_cache_is_scoped_to_the_publishing_channel() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("viewed-notices-v2");
        let now = at("2026-08-20T10:30:00Z");
        let mut cache = ViewedNotices::default();
        cache.insert(
            ("https://a.example.com/c".to_owned(), "1".to_owned()),
            viewed("https://a.example.com/c", "1", "2026-08-20T10:30:00Z"),
        );
        write_viewed_channel_notices_to(&path, &cache, now).unwrap();

        let stored = read_viewed_channel_notices_from(&path);
        let other = ChannelNotice {
            id: "1".to_owned(),
            message: "channel B".to_owned(),
            level: ChannelNoticeLevel::Critical,
            created_at: None,
            expires_at: None,
            interval: None,
        };
        assert!(should_display(
            stored.get(&("https://b.example.com/c".to_owned(), "1".to_owned())),
            &other,
            now
        ));
    }

    /// A newline in a channel-supplied id must not become a second entry, which
    /// would let one channel mark another channel's notice as already seen.
    #[test]
    fn an_id_containing_a_newline_cannot_forge_another_entry() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("viewed-notices-v2");
        let now = at("2026-08-20T10:30:00Z");
        let mut cache = ViewedNotices::default();
        cache.insert(
            (
                "https://evil.example.com/c".to_owned(),
                "x\nsecurity-1".to_owned(),
            ),
            viewed(
                "https://evil.example.com/c",
                "x\nsecurity-1",
                "2026-08-20T10:30:00Z",
            ),
        );
        write_viewed_channel_notices_to(&path, &cache, now).unwrap();

        let stored = read_viewed_channel_notices_from(&path);
        assert_eq!(stored.len(), 1);
        assert!(stored.contains_key(&(
            "https://evil.example.com/c".to_owned(),
            "x\nsecurity-1".to_owned()
        )));
    }

    #[test]
    fn an_empty_id_is_recorded_like_any_other() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("viewed-notices-v2");
        let now = at("2026-08-20T10:30:00Z");
        let mut cache = ViewedNotices::default();
        cache.insert(
            ("https://example.com/c".to_owned(), String::new()),
            viewed("https://example.com/c", "", "2026-08-20T10:30:00Z"),
        );
        write_viewed_channel_notices_to(&path, &cache, now).unwrap();

        assert_eq!(read_viewed_channel_notices_from(&path).len(), 1);
    }

    #[test]
    fn a_damaged_cache_reads_as_empty_rather_than_suppressing_notices() {
        let temp_dir = tempfile::tempdir().unwrap();
        for contents in ["", "not json", "{\"unexpected\": true}", "\u{0}\u{1}\u{2}"] {
            let path = temp_dir.path().join("viewed-notices-v2");
            fs_err::write(&path, contents).unwrap();
            assert!(read_viewed_channel_notices_from(&path).is_empty());
        }
    }

    #[test]
    fn a_missing_cache_reads_as_empty() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("does/not/exist");
        assert!(read_viewed_channel_notices_from(&path).is_empty());
    }

    #[test]
    fn writing_merges_with_what_another_process_recorded() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("viewed-notices-v2");
        let now = at("2026-08-20T10:30:00Z");

        let mut first = ViewedNotices::default();
        first.insert(
            ("https://example.com/c".to_owned(), "a".to_owned()),
            viewed("https://example.com/c", "a", "2026-08-20T10:30:00Z"),
        );
        write_viewed_channel_notices_to(&path, &first, now).unwrap();

        let mut second = ViewedNotices::default();
        second.insert(
            ("https://example.com/c".to_owned(), "b".to_owned()),
            viewed("https://example.com/c", "b", "2026-08-20T10:30:00Z"),
        );
        write_viewed_channel_notices_to(&path, &second, now).unwrap();

        let stored = read_viewed_channel_notices_from(&path);
        assert_eq!(
            stored.len(),
            2,
            "an id recorded by another process was lost"
        );
    }

    #[test]
    fn a_write_failure_is_reported_rather_than_silently_skipped() {
        let temp_dir = tempfile::tempdir().unwrap();
        let blocker = temp_dir.path().join("notices");
        fs_err::write(&blocker, "not a directory").unwrap();

        let result = write_viewed_channel_notices_to(
            &blocker.join("viewed-notices-v2"),
            &ViewedNotices::default(),
            Timestamp::now(),
        );
        assert!(result.is_err());
    }

    // -- Re-display policy -------------------------------------------------

    #[test]
    fn a_notice_without_an_interval_is_shown_once() {
        let now = at("2026-08-20T10:30:00Z");
        let seen = viewed("https://example.com/c", "1", "2026-08-20T10:00:00Z");
        let notice = ChannelNotice {
            id: "1".to_owned(),
            message: "hello".to_owned(),
            level: ChannelNoticeLevel::Info,
            created_at: None,
            expires_at: None,
            interval: None,
        };

        assert!(should_display(None, &notice, now));
        assert!(!should_display(Some(&seen), &notice, now));
    }

    /// CEP-6 lets a channel ask for a notice to be repeated every `interval`
    /// seconds.
    #[test]
    fn an_interval_brings_a_notice_back() {
        let now = at("2026-08-20T10:30:00Z");
        let seen = viewed("https://example.com/c", "1", "2026-08-20T10:00:00Z");
        let mut notice = ChannelNotice {
            id: "1".to_owned(),
            message: "hello".to_owned(),
            level: ChannelNoticeLevel::Critical,
            created_at: None,
            expires_at: None,
            interval: Some(60 * 60),
        };
        assert!(!should_display(Some(&seen), &notice, now));

        notice.interval = Some(60);
        assert!(should_display(Some(&seen), &notice, now));
    }

    #[test]
    fn an_absurd_interval_does_not_overflow() {
        let now = at("2026-08-20T10:30:00Z");
        let seen = viewed("https://example.com/c", "1", "2026-08-20T10:00:00Z");
        let notice = ChannelNotice {
            id: "1".to_owned(),
            message: "hello".to_owned(),
            level: ChannelNoticeLevel::Info,
            created_at: None,
            expires_at: None,
            interval: Some(u64::MAX),
        };

        assert!(!should_display(Some(&seen), &notice, now));
    }

    // -- Pruning -----------------------------------------------------------

    #[test]
    fn expired_and_stale_entries_are_forgotten() {
        let now = at("2026-08-20T10:30:00Z");
        let mut cache = ViewedNotices::default();

        let mut expired = viewed("https://example.com/c", "expired", "2026-08-01T10:30:00Z");
        expired.expires_at = Some(at("2026-08-10T10:30:00Z"));
        cache.insert(
            ("https://example.com/c".to_owned(), "expired".to_owned()),
            expired,
        );

        let mut live = viewed("https://example.com/c", "live", "2026-08-19T10:30:00Z");
        live.expires_at = Some(at("2026-09-30T10:30:00Z"));
        cache.insert(
            ("https://example.com/c".to_owned(), "live".to_owned()),
            live,
        );

        cache.insert(
            ("https://example.com/c".to_owned(), "stale".to_owned()),
            viewed("https://example.com/c", "stale", "2020-01-01T00:00:00Z"),
        );
        cache.insert(
            ("https://example.com/c".to_owned(), "recent".to_owned()),
            viewed("https://example.com/c", "recent", "2026-08-19T10:30:00Z"),
        );

        prune_viewed_channel_notices(&mut cache, now);

        let mut remaining = cache.keys().map(|(_, id)| id.as_str()).collect::<Vec<_>>();
        remaining.sort_unstable();
        assert_eq!(remaining, vec!["live", "recent"]);
    }
}
