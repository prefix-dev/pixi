use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle, style::ProgressTracker};
use parking_lot::RwLock;
use pixi_progress::ProgressBarPlacement;

#[derive(Clone)]
pub struct MainProgressBar<T> {
    inner: Arc<RwLock<State<T>>>,
}

/// Holds the internal state of a [`MainProgressBar`].
struct State<T> {
    /// The multi progress instance that is used to manage the progress bar.
    multi_progress: MultiProgress,

    /// The progress bar that is being used to display the progress.
    pb: ProgressBar,

    /// The title of the progress bar, only Some if the progress bar is
    /// invisible.
    title: Option<String>,

    /// The items that are being tracked by this progress bar.
    tracker: Arc<RwLock<HashMap<usize, TrackedItem<T>>>>,
    next_tracker_id: usize,
}

/// A trait for something that can be tracked by the [`MainProgressBar`].
pub trait Tracker: Ord + Send + Sync + 'static {
    /// Returns the name of the item being tracked.
    fn name(&self) -> &str;

    /// Returns the size of the item being tracked.
    fn size(&self) -> u64 {
        1
    }
}

impl Tracker for String {
    fn name(&self) -> &str {
        self.as_str()
    }
}

/// Internal state that tracks information about an item being tracked by the
/// [`MainProgressBar`].
struct TrackedItem<T> {
    tracker: T,
    started: Option<Instant>,
    finished: Option<Instant>,
}

impl<T: Tracker> MainProgressBar<T> {
    /// Constructs a new instance of [`MainProgressBar`] with the given title
    /// and placement.
    pub fn new(
        multi_progress: MultiProgress,
        progress_bar_placement: ProgressBarPlacement,
        title: String,
    ) -> Self {
        let pb = progress_bar_placement.insert(multi_progress.clone(), ProgressBar::hidden());
        Self {
            inner: Arc::new(RwLock::new(State {
                multi_progress,
                pb,
                title: Some(title),
                tracker: Arc::new(RwLock::new(HashMap::new())),
                next_tracker_id: 0,
            })),
        }
    }

    /// Called when an item is queued for processing.
    pub fn queued(&self, tracker: T) -> usize {
        let mut state = self.inner.write();
        state.queued(tracker)
    }

    /// Called when processing of an item is started.
    pub fn start(&self, id: usize) {
        let mut state = self.inner.write();
        state.start(id)
    }

    /// Called when processing of an item has finished.
    pub fn finish(&self, id: usize) {
        let mut state = self.inner.write();
        state.finish(id);
    }

    /// Called to clear the progress bar.
    pub fn clear(&self) {
        let mut state = self.inner.write();
        state.clear();
    }
}

impl<T: Tracker> State<T> {
    pub fn clear(&mut self) {
        let mut trackers = self.tracker.write();

        // If the tracker is already empty, we don't need to do anything.
        if trackers.is_empty() {
            return;
        }

        // Clear all items that have finished processing.
        trackers.retain(|_, item| item.finished.is_none());

        // Drop the write lock to the trackers. The progress bar also requires access to
        // entries so holding on to the write lock might deadlock!
        let is_empty = trackers.is_empty();
        drop(trackers);

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

    pub fn start(&mut self, id: usize) {
        let mut trackers = self.tracker.write();
        trackers.get_mut(&id).expect("missing tracker id").started = Some(Instant::now());
        drop(trackers);
        self.update();
    }

    pub fn finish(&mut self, id: usize) {
        let mut trackers = self.tracker.write();
        let tracker = trackers.get_mut(&id).expect("missing tracker id");
        if tracker.finished.is_none() {
            let now = Instant::now();
            tracker.started.get_or_insert(now);
            tracker.finished = Some(now);
            drop(trackers);
            self.update();
        }
    }

    pub fn queued(&mut self, tracker: T) -> usize {
        let mut trackers = self.tracker.write();
        let id = self.next_tracker_id;
        self.next_tracker_id += 1;
        trackers.insert(
            id,
            TrackedItem {
                tracker,
                started: None,
                finished: None,
            },
        );
        drop(trackers);
        self.update();
        id
    }

    pub fn update(&mut self) {
        // If there are no trackers, we don't need to update the progress bar at all.
        if self.next_tracker_id == 0 {
            return;
        }

        let tracker = self.tracker.read();

        let length: u64 = tracker.values().map(|item| item.tracker.size()).sum();
        let position: u64 = tracker
            .values()
            .filter(|item| item.finished.is_some())
            .map(|item| item.tracker.size())
            .sum();

        // Create a list of currently running items.
        let mut seen_items = HashSet::new();
        let running_items = tracker
            .values()
            .filter(|item| item.started.is_some() && item.finished.is_none())
            .filter(|item| seen_items.insert(item.tracker.name()))
            .collect::<Vec<_>>();
        let first = running_items
            .iter()
            .max_by(|a, b| a.tracker.cmp(&b.tracker));
        let wide_msg = match (first, running_items.len()) {
            (None, _) => String::new(),
            (Some(first), 1) => first.tracker.name().to_string(),
            (Some(first), _) => {
                format!("{} (+{})", first.tracker.name(), running_items.len() - 1,)
            }
        };

        // Set the style of the progress bar.
        let active = !running_items.is_empty();
        self.pb.set_style(
            ProgressStyle::with_template(
                &format!("{{spinner:.{spinner}}} {{prefix:20!}} [{{bar:20!.bright.yellow/dim.white}}] {{pos_count:>2.dim}}{slash}{{len_count:2.dim}} {{wide_msg:.dim}}",
                         spinner = if running_items.is_empty() { "dim" } else { "green" },
                         slash = console::style("/").dim(),
                ))
                .expect("failed to create progress bar style")
                .tick_chars(pixi_progress::style::tick_chars(active))
                .progress_chars(pixi_progress::style::progress_chars(active))
                .with_key("pos_count", PosCount::new(self.tracker.clone()))
                .with_key("len_count", TotalCount::new(self.tracker.clone()))
        );
        if let Some(title) = self.title.take() {
            self.pb.set_prefix(title);
            self.pb.enable_steady_tick(Duration::from_millis(100));
        }
        self.pb.update(|state| {
            state.set_len(length);
            state.set_pos(position);
        });
        self.pb.set_message(wide_msg);
    }
}

struct PosCount<T> {
    trackers: Arc<RwLock<HashMap<usize, TrackedItem<T>>>>,
    count: usize,
}

impl<T> PosCount<T> {
    pub fn new(trackers: Arc<RwLock<HashMap<usize, TrackedItem<T>>>>) -> Self {
        Self { trackers, count: 0 }
    }
}

impl<T: Send + Sync + 'static> ProgressTracker for PosCount<T> {
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
            .filter(|item| item.finished.is_some())
            .count();
    }
    fn reset(&mut self, _state: &ProgressState, _now: Instant) {}
    fn write(&self, _state: &ProgressState, w: &mut dyn fmt::Write) {
        write!(w, "{}", self.count).expect("failed to write progress count");
    }
}

struct TotalCount<T> {
    trackers: Arc<RwLock<HashMap<usize, TrackedItem<T>>>>,
    count: usize,
}

impl<T> TotalCount<T> {
    pub fn new(trackers: Arc<RwLock<HashMap<usize, TrackedItem<T>>>>) -> Self {
        Self { trackers, count: 0 }
    }
}

impl<T: Send + Sync + 'static> ProgressTracker for TotalCount<T> {
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
    fn write(&self, _state: &ProgressState, w: &mut dyn fmt::Write) {
        write!(w, "{}", self.count).expect("failed to write progress count");
    }
}
