use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use parking_lot::RwLock;
use pixi_progress::ProgressBarPlacement;

pub struct MainProgressBar<T> {
    inner: Arc<RwLock<State<T>>>,
}

struct State<T> {
    /// The progress bar that is being used to display the progress.
    pb: ProgressBar,

    /// The title of the progress bar, only Some if the progress bar is
    /// invisible.
    title: Option<String>,

    /// The items that are being tracked by this progress bar.
    tracker: HashMap<usize, TrackedItem<T>>,
    next_tracker_id: usize,
}

pub trait Tracker: Ord {
    /// Returns the name of the item being tracked.
    fn name(&self) -> &str;
}

impl Tracker for String {
    fn name(&self) -> &str {
        self.as_str()
    }
}

struct TrackedItem<T> {
    tracker: T,
    started: Option<Instant>,
    finished: Option<Instant>,
}

impl<T: Tracker> MainProgressBar<T> {
    pub fn new(
        multi_progress: MultiProgress,
        progress_bar_placement: ProgressBarPlacement,
        title: String,
    ) -> Self {
        let pb = progress_bar_placement.insert(multi_progress, ProgressBar::hidden());
        Self {
            inner: Arc::new(RwLock::new(State {
                pb,
                title: Some(title),
                tracker: HashMap::new(),
                next_tracker_id: 0,
            })),
        }
    }

    pub fn queued(&self, tracker: T) -> usize {
        let mut state = self.inner.write();
        state.queued(tracker)
    }

    pub fn start(&self, id: usize) {
        let mut state = self.inner.write();
        state.start(id)
    }

    pub fn finish(&self, id: usize) {
        let mut state = self.inner.write();
        state.finish(id);
    }

    pub fn clear(&self) {
        let mut state = self.inner.write();
        state.close();
    }
}

impl<T: Tracker> State<T> {
    pub fn close(&mut self) {
        self.pb.finish_and_clear();
        self.tracker.clear();
    }

    pub fn start(&mut self, id: usize) {
        self.tracker
            .get_mut(&id)
            .expect("missing tracker id")
            .started = Some(Instant::now());
        self.update();
    }

    pub fn finish(&mut self, id: usize) {
        self.tracker
            .get_mut(&id)
            .expect("missing tracker id")
            .finished = Some(Instant::now());
        self.update();
    }

    pub fn queued(&mut self, tracker: T) -> usize {
        let id = self.next_tracker_id;
        self.next_tracker_id += 1;
        self.tracker.insert(
            id,
            TrackedItem {
                tracker,
                started: None,
                finished: None,
            },
        );
        self.update();
        id
    }

    pub fn update(&mut self) {
        // If there are no trackers, we don't need to update the progress bar at all.
        if self.next_tracker_id == 0 {
            return;
        }

        // Make the progress bar visible if it is not already.
        if let Some(title) = self.title.take() {
            self.pb.set_prefix(title);
            self.pb.enable_steady_tick(Duration::from_millis(100));
        }

        let total = self.tracker.len();
        let finished = self
            .tracker
            .values()
            .filter(|item| item.finished.is_some())
            .count();

        // Create a list of currently running items.
        let mut seen_items = HashSet::new();
        let running_items = self
            .tracker
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
        self.pb.set_style(
            ProgressStyle::with_template(
                &format!("{{spinner:.{spinner}}} {{prefix:20!}} [{{bar:20!.{bar}.yellow/dim.white}}] {{pos:>2.dim}}{slash}{{len:2.dim}} {{wide_msg:.dim}}",
                    spinner = if running_items.is_empty() { "dim" } else { "green" },
                    slash = console::style("/").dim(),
                    bar = if running_items.is_empty() { "dim" } else { "bright" },
                ))
                .expect("failed to create progress bar style")
                .tick_chars(if running_items.is_empty() { "▪▪" } else { "⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈ " })
                .progress_chars("━━╾─"),
            );
        self.pb.set_length(total as u64);
        self.pb.set_position(finished as u64);
        self.pb.set_message(wide_msg);
    }
}
