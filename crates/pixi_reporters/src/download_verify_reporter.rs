use std::{
    borrow::Cow,
    collections::HashMap,
    fmt::Write,
    sync::Arc,
    time::{Duration, Instant},
};

use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle, style::ProgressTracker};
use parking_lot::RwLock;
use pixi_command_dispatcher::SourceBuildSpec;
use pixi_progress::ProgressBarPlacement;
use rattler_conda_types::RepoDataRecord;

#[derive(Clone)]
pub struct BuildDownloadVerifyReporter {
    multi_progress: MultiProgress,
    pb: ProgressBar,
    title: Option<String>,
    entries: Arc<RwLock<HashMap<usize, Entry>>>,
    next_entry_id: usize,
}

#[derive(Debug)]
struct Entry {
    name: String,
    size: Option<u64>,
    state: EntryState,
}

#[derive(Debug)]
pub enum EntryState {
    Building,
    Pending,
    Validating,
    Downloading {
        started: Instant,
        total_bytes: Option<u64>,
        bytes_downloaded: u64,
    },
    Finished {
        download: Option<(Instant, Instant, u64)>,
    },
}

impl Entry {
    pub fn is_finished(&self) -> bool {
        matches!(&self.state, EntryState::Finished { .. })
    }

    pub fn is_downloading(&self) -> bool {
        matches!(&self.state, EntryState::Downloading { .. })
    }

    pub fn is_validating(&self) -> bool {
        matches!(&self.state, EntryState::Validating)
    }

    pub fn is_building(&self) -> bool {
        matches!(&self.state, EntryState::Building)
    }

    pub fn is_active(&self) -> bool {
        self.is_downloading() || self.is_validating() || self.is_building()
    }

    pub fn progress(&self) -> u64 {
        match &self.state {
            EntryState::Downloading {
                bytes_downloaded, ..
            } => *bytes_downloaded,
            EntryState::Finished {
                download: Some((_, _, size)),
            } => *size,
            EntryState::Finished { download: None } => self.size.unwrap_or(1),
            _ => 0,
        }
    }

    pub fn size(&self) -> u64 {
        match &self.state {
            EntryState::Downloading { total_bytes, .. } => total_bytes.or(self.size).unwrap_or(1),
            EntryState::Finished {
                download: Some((_, _, size)),
            } => *size,
            _ => self.size.unwrap_or(1),
        }
    }
}

impl BuildDownloadVerifyReporter {
    pub fn new(
        multi_progress: MultiProgress,
        progress_bar_placement: ProgressBarPlacement,
        title: String,
    ) -> Self {
        let pb = progress_bar_placement.insert(multi_progress.clone(), ProgressBar::hidden());
        Self {
            multi_progress,
            pb,
            title: Some(title),
            entries: Arc::new(RwLock::new(HashMap::new())),
            next_entry_id: 0,
        }
    }

    pub fn progress_bar(&self) -> ProgressBar {
        self.pb.clone()
    }

    pub fn clear(&mut self) {
        let mut entries = self.entries.write();

        // If the tracker is already empty, we don't need to do anything.
        if entries.is_empty() {
            return;
        }

        // Clear all items that have finished processing.
        entries.retain(|_, item| !item.is_finished());

        // Drop the write lock to the entries. The progress bar also requires access to
        // entries so holding on to the write lock might deadlock!
        let is_empty = entries.is_empty();
        drop(entries);

        // Clear or update the progress bar.
        if is_empty {
            // We cannot clear the progress bar and restart it later, so replacing it with a
            // new hidden one is currently the only option.
            self.title = Some(self.pb.prefix());
            let new_pb = self
                .multi_progress
                .insert_after(&self.pb, ProgressBar::hidden());
            self.pb.finish_and_clear();
            self.pb = new_pb;
        } else {
            self.update()
        }
    }

    pub fn on_build_queued(&mut self, spec: &SourceBuildSpec) -> usize {
        let mut entries = self.entries.write();
        let id = self.next_entry_id;
        self.next_entry_id += 1;
        entries.insert(
            id,
            Entry {
                name: format!("building {}", spec.package.name.as_source()),
                size: None,
                state: EntryState::Pending,
            },
        );
        drop(entries);
        self.update();
        id
    }

    pub fn on_build_start(&mut self, index: usize) {
        let mut entries = self.entries.write();
        entries
            .get_mut(&index)
            .expect("entry is missing from tracker")
            .state = EntryState::Building;
        drop(entries);
        self.update();
    }

    pub fn on_build_finished(&mut self, index: usize) {
        let mut entries = self.entries.write();
        let entry = entries
            .get_mut(&index)
            .expect("entry is missing from tracker");
        match entry.state {
            EntryState::Building => {
                entry.state = EntryState::Finished { download: None };
            }
            EntryState::Pending | EntryState::Validating => {
                entry.state = EntryState::Finished { download: None };
            }
            _ => {}
        };
        drop(entries);
        self.update();
    }

    pub fn on_entry_start(&mut self, record: &RepoDataRecord) -> usize {
        let mut entries = self.entries.write();
        let id = self.next_entry_id;
        self.next_entry_id += 1;
        entries.insert(
            id,
            Entry {
                name: record.package_record.name.as_normalized().to_string(),
                size: record.package_record.size,
                state: EntryState::Pending,
            },
        );
        drop(entries);
        self.update();
        id
    }

    pub fn on_validation_start(&mut self, index: usize) {
        let mut entries = self.entries.write();
        entries
            .get_mut(&index)
            .expect("entry is missing from tracker")
            .state = EntryState::Validating;
        drop(entries);
        self.update();
    }

    pub fn on_validation_complete(&mut self, index: usize) {
        let mut entries = self.entries.write();
        let entry = entries
            .get_mut(&index)
            .expect("entry is missing from tracker");
        let EntryState::Validating = entry.state else {
            panic!(
                "Expected entry to be in downloading state, actual: {:?}",
                entry
            );
        };
        entry.state = EntryState::Finished { download: None };
        drop(entries);
        self.update();
    }

    pub fn on_download_start(&mut self, index: usize) {
        let mut entries = self.entries.write();
        let entry = entries
            .get_mut(&index)
            .expect("entry is missing from tracker");
        entry.state = EntryState::Downloading {
            started: Instant::now(),
            total_bytes: None,
            bytes_downloaded: 0,
        };
        drop(entries);
        self.update();
    }

    pub fn on_download_progress(
        &mut self,
        index: usize,
        new_bytes_downloaded: u64,
        new_total_bytes: Option<u64>,
    ) {
        let mut entries = self.entries.write();
        let entry = entries
            .get_mut(&index)
            .expect("entry is missing from tracker");
        let EntryState::Downloading {
            total_bytes,
            bytes_downloaded,
            ..
        } = &mut entry.state
        else {
            panic!("Expected entry to be in downloading state");
        };
        if let Some(new_total_bytes) = new_total_bytes {
            total_bytes.get_or_insert(new_total_bytes);
        }
        *bytes_downloaded = new_bytes_downloaded;
        drop(entries);
        self.update();
    }

    pub fn on_download_complete(&mut self, index: usize) {
        let mut entries = self.entries.write();
        let entry = entries
            .get_mut(&index)
            .expect("entry is missing from tracker");
        let EntryState::Downloading {
            total_bytes,
            bytes_downloaded,
            started,
        } = &mut entry.state
        else {
            panic!("Expected entry to be in downloading state");
        };
        entry.state = EntryState::Finished {
            download: Some((
                *started,
                Instant::now(),
                total_bytes.unwrap_or(*bytes_downloaded),
            )),
        };
        drop(entries);
        self.update();
    }

    pub fn on_entry_finished(&mut self, index: usize) {
        let mut entries = self.entries.write();
        let entry = entries
            .get_mut(&index)
            .expect("entry is missing from tracker");
        match &entry.state {
            EntryState::Downloading {
                bytes_downloaded,
                total_bytes,
                started,
            } => {
                entry.state = EntryState::Finished {
                    download: Some((
                        *started,
                        Instant::now(),
                        total_bytes.unwrap_or(*bytes_downloaded),
                    )),
                };
            }
            EntryState::Pending | EntryState::Validating => {
                entry.state = EntryState::Finished { download: None };
            }
            _ => {}
        };
        drop(entries);
        self.update();
    }

    fn update(&mut self) {
        let entries = self.entries.read();
        if !entries.values().any(|d| d.is_active() || d.is_finished()) {
            // Don't do anything if nothing has started.
            return;
        }

        let total_bytes = entries.values().map(|d| d.size()).sum::<u64>();
        let bytes_downloaded = entries.values().map(|d| d.progress()).sum::<u64>();

        // Find the biggest pending entry
        let (first, running_count, is_downloading) = find_max_and_multiple(entries.values());
        let wide_msg = match (first, running_count) {
            (None, _) => Cow::Borrowed(""),
            (Some(first), 1) => Cow::Borrowed(first.name.as_str()),
            (Some(first), running_count) => {
                Cow::Owned(format!("{} (+{})", &first.name, running_count - 1,))
            }
        };
        let has_pending_entries = running_count > 0;

        // Set the style of the progress bar.
        let verbose = tracing::event_enabled!(tracing::Level::INFO);
        self.pb.set_style(
            ProgressStyle::with_template(&format!(
                "{{spinner:.{spinner}}} {{prefix:20!}} [{{bar:20!.bright.yellow/dim.white}}] {{pos_count:>2.dim}}{slash}{{len_count:2.dim}} {{msg:.dim}} {verbose}{speed}",
                spinner = if has_pending_entries { "green" } else { "dim" },
                slash = console::style("/").dim(),
                verbose = if verbose { format!("{{bytes:>2.dim}}{slash}{{total_bytes:>2.dim}} ", slash = console::style("/").dim()) } else { String::new() },
                speed = if is_downloading { format!("{at} {{speed:.dim}}", at = console::style("@").dim()) } else { String::new() }
            ))
                .expect("failed to create progress bar style")
                .tick_chars(pixi_progress::style::tick_chars(has_pending_entries))
                .progress_chars(pixi_progress::style::progress_chars(has_pending_entries))
                .with_key(
                    "speed",
                    DurationTracker::new(self.entries.clone()),
                )
                .with_key("pos_count", PosCount::new(self.entries.clone()))
                .with_key("len_count", TotalCount::new(self.entries.clone())),
        );

        // Set the title of the progress bar if it is was missing
        if let Some(title) = self.title.take() {
            self.pb.set_prefix(title);
            self.pb.enable_steady_tick(Duration::from_millis(100));
        }
        self.pb.update(|state| {
            state.set_pos(bytes_downloaded);
            state.set_len(total_bytes);
        });
        self.pb.set_message(wide_msg.into_owned());
    }
}

/// Compute the total active time of all downloads.
///
/// This is useful for calculating the average download speed in a situation
/// where there could also not be a download active for a period of time.
///
/// The function calculates the total active download time from a slice of
/// `Entry` items, considering their start and finish times, and
/// returns the result as a `Duration`.
fn total_duration_and_size<'a>(
    items: impl IntoIterator<Item = &'a Entry>,
    now: Instant,
) -> (Duration, u64) {
    let mut intervals: Vec<(Instant, Instant, u64)> = items
        .into_iter()
        .filter_map(|d| match d.state {
            EntryState::Downloading {
                started,
                bytes_downloaded,
                ..
            } => Some((started, now, bytes_downloaded)),
            EntryState::Finished {
                download: Some((started, finished, size)),
            } => Some((started, finished, size)),
            _ => None,
        })
        .collect();

    // Sort intervals by start time
    intervals.sort_by_key(|(start, _, _)| *start);

    let mut total = Duration::ZERO;
    let mut current: Option<(Instant, Instant)> = None;
    let mut total_size = 0;

    for (start, end, size) in intervals {
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
        total_size += size;
    }

    if let Some((cur_start, cur_end)) = current {
        total += cur_end.duration_since(cur_start);
    }

    (total, total_size)
}

fn find_max_and_multiple<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
) -> (Option<&'a Entry>, usize, bool) {
    let mut iter = entries.into_iter().filter(|entry| entry.is_active());
    let Some(mut max) = iter.next() else {
        return (None, 0, false);
    };
    let mut is_downloading = max.is_downloading();
    let mut count = 1;
    for next in iter {
        count += 1;
        if next.size() > max.size() {
            max = next;
        }
        is_downloading |= next.is_downloading();
    }
    (Some(max), count, is_downloading)
}

/// This is a custom progress tracker that calculates the average download speed
/// while taking into account the total active time of all downloads.
#[derive(Clone)]
pub(super) struct DurationTracker {
    inner: Arc<RwLock<HashMap<usize, Entry>>>,
    duration: Duration,
    len: u64,
}

impl DurationTracker {
    fn new(inner: Arc<RwLock<HashMap<usize, Entry>>>) -> Self {
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

    fn tick(&mut self, _state: &ProgressState, now: std::time::Instant) {
        let inner = self.inner.read();
        let (duration, len) = total_duration_and_size(inner.values(), now);
        self.duration = duration;
        self.len = len;
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

struct PosCount {
    trackers: Arc<RwLock<HashMap<usize, Entry>>>,
    count: usize,
}

impl PosCount {
    pub fn new(trackers: Arc<RwLock<HashMap<usize, Entry>>>) -> Self {
        Self { trackers, count: 0 }
    }
}

impl ProgressTracker for PosCount {
    fn clone_box(&self) -> Box<dyn ProgressTracker> {
        Box::new(PosCount {
            trackers: Arc::clone(&self.trackers),
            count: self.count,
        })
    }
    fn tick(&mut self, _state: &ProgressState, _now: Instant) {
        self.count = self
            .trackers
            .read()
            .values()
            .filter(|item| item.is_finished())
            .count();
    }
    fn reset(&mut self, _state: &ProgressState, _now: Instant) {}
    fn write(&self, _state: &ProgressState, w: &mut dyn Write) {
        write!(w, "{}", self.count).expect("failed to write progress count");
    }
}

struct TotalCount {
    trackers: Arc<RwLock<HashMap<usize, Entry>>>,
    count: usize,
}

impl TotalCount {
    pub fn new(trackers: Arc<RwLock<HashMap<usize, Entry>>>) -> Self {
        Self { trackers, count: 0 }
    }
}

impl ProgressTracker for TotalCount {
    fn clone_box(&self) -> Box<dyn ProgressTracker> {
        Box::new(TotalCount {
            trackers: Arc::clone(&self.trackers),
            count: self.count,
        })
    }
    fn tick(&mut self, _state: &ProgressState, _now: Instant) {
        self.count = self.trackers.read().len();
    }
    fn reset(&mut self, _state: &ProgressState, _now: Instant) {}
    fn write(&self, _state: &ProgressState, w: &mut dyn Write) {
        write!(w, "{}", self.count).expect("failed to write progress count");
    }
}
