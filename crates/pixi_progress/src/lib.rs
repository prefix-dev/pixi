mod placement;
pub mod style;

use std::{
    borrow::Cow,
    fmt::Write,
    future::Future,
    sync::{Arc, LazyLock},
    time::Duration,
};

use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressDrawTarget, ProgressState};
use parking_lot::Mutex;
pub use placement::ProgressBarPlacement;

/// A helper macro to print a message to the console. If a multi-progress bar
/// is currently active, this macro will suspend the progress bar, print the
/// message and continue the progress bar. This ensures the output does not
/// interfere with the progress bar.
///
/// If the progress bar is hidden, the message will be printed to `stderr`
/// instead.
#[macro_export]
macro_rules! println {
    () => {
        let mp = $crate::global_multi_progress();
        if mp.is_hidden() {
            eprintln!();
        } else {
            // Ignore any error
            let _err = mp.println("");
        }
    };
    ($($arg:tt)*) => {
        let mp = $crate::global_multi_progress();
        if mp.is_hidden() {
            eprintln!($($arg)*);
        } else {
            // Ignore any error
            let _err = mp.println(format!($($arg)*));
        }
    }
}

/// Returns a global instance of [`indicatif::MultiProgress`].
///
/// Although you can always create an instance yourself any logging will
/// interrupt pending progressbars. To fix this issue, logging has been
/// configured in such a way to it will not interfere if you use the
/// [`indicatif::MultiProgress`] returning by this function.
pub fn global_multi_progress() -> MultiProgress {
    static GLOBAL_MP: LazyLock<MultiProgress> = LazyLock::new(|| {
        let mp = MultiProgress::new();
        mp.set_draw_target(ProgressDrawTarget::stderr_with_hz(20));
        mp
    });
    GLOBAL_MP.clone()
}

/// Returns the style to use for a progressbar that is currently in progress.
pub fn default_bytes_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template("  {spinner:.dim} {prefix:20!} [{elapsed_precise}] [{bar:20!.bright.yellow/dim.white}] {bytes:>8} @ {smoothed_bytes_per_sec:8}").unwrap()
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
}

/// Returns the style to use for a progressbar that is currently in progress.
pub fn default_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template("  {spinner:.dim} {prefix:20!} [{elapsed_precise}] [{bar:20!.bright.yellow/dim.white}] {pos:>4}/{len:4} {wide_msg:.dim}").unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is indeterminate and simply
/// shows a spinner.
pub fn long_running_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::with_template("{prefix}{spinner:.green} {msg}").unwrap()
}

/// Displays a spinner with the given message while running the specified
/// function to completion.
pub fn wrap_in_progress<T, F: FnOnce() -> T>(msg: impl Into<Cow<'static, str>>, func: F) -> T {
    let pb = global_multi_progress().add(ProgressBar::new_spinner());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(long_running_progress_style());
    pb.set_message(msg);
    let result = func();
    pb.finish_and_clear();
    result
}

/// Displays a spinner with the given message while running the specified
/// function to completion.
pub async fn await_in_progress<T, F: FnOnce(ProgressBar) -> Fut, Fut: Future<Output = T>>(
    msg: impl Into<Cow<'static, str>>,
    future: F,
) -> T {
    let msg = msg.into();
    let (prefix, msg) = match msg.find(|c: char| !c.is_whitespace()) {
        Some(idx) if idx > 0 => msg.split_at(idx),
        _ => ("", msg.as_ref()),
    };

    let pb = global_multi_progress().add(ProgressBar::new_spinner());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(long_running_progress_style());
    pb.set_prefix(prefix.to_string());
    pb.set_message(msg.to_string());
    let result = future(pb.clone()).await;
    pb.finish_and_clear();
    result
}

/// Style an existing progress bar with a warning style and the given message.
pub fn style_warning_pb(pb: ProgressBar, warning_msg: String) -> ProgressBar {
    pb.set_style(
        indicatif::ProgressStyle::default_spinner() // Or default_bar() if you used ProgressBar::new(length)
            .template("  {spinner:.yellow} {wide_msg:.yellow}") // Yellow spinner, clear message
            .expect("failed to set a progress bar template"),
    );
    pb.set_message(warning_msg);
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

/// A struct that can be used to format the message part of a progress bar.
///
/// It's primary usecase is when you have a single progress bar but multiple
/// tasks that are running and which you want to communicate to the user. This
/// struct will set the message part of the passed progress bar to the oldest
/// unfinished task and include a the number of pending tasks.
#[derive(Debug)]
pub struct ProgressBarMessageFormatter {
    state: Arc<Mutex<State>>,
}

/// Internal state kept by the [`ProgressBarMessageFormatter`] and derived
/// state.
///
/// This contains the state of the formatter and allows updating the progress
/// bar.
#[derive(Debug)]
struct State {
    pb: ProgressBar,
    pending: Vec<String>,
}

impl State {
    /// Notify the state that a certain operation happened.
    fn notify(&mut self, msg: Operation) {
        match msg {
            Operation::Started(op) => self.pending.push(op),
            Operation::Finished(op) => {
                let Some(idx) = self.pending.iter().position(|p| p == &op) else {
                    panic!("operation {op} was never started");
                };
                self.pending.remove(idx);
            }
        }

        if self.pending.is_empty() {
            self.pb.set_message("");
        } else if self.pending.len() == 1 {
            self.pb.set_message(self.pending[0].clone());
        } else {
            self.pb.set_message(format!(
                "{} (+{})",
                self.pending.last().unwrap(),
                self.pending.len() - 1
            ));
        }
    }
}

#[derive(Debug)]
enum Operation {
    Started(String),
    Finished(String),
}

pub struct ScopedTask {
    state: Option<Arc<Mutex<State>>>,
    name: String,
}

impl ScopedTask {
    fn start(name: String, state: Arc<Mutex<State>>) -> Self {
        state.lock().notify(Operation::Started(name.clone()));
        Self {
            state: Some(state),
            name,
        }
    }

    /// Finishes the execution of the task.
    pub fn finish(self) {
        drop(self)
    }
}

impl Drop for ScopedTask {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            state
                .lock()
                .notify(Operation::Finished(std::mem::take(&mut self.name)));
        }
    }
}

impl ProgressBarMessageFormatter {
    /// Allows the user to specify a custom capacity for the internal channel.
    pub fn new(pb: ProgressBar) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                pb,
                pending: Vec::new(),
            })),
        }
    }

    /// Adds the start of another task to the progress bar and returns an object
    /// that is used to mark the lifetime of the task. If the object is
    /// dropped the task is considered finished.
    #[must_use]
    pub fn start(&self, op: String) -> ScopedTask {
        ScopedTask::start(op, self.state.clone())
    }

    /// Wraps an future into a task which starts when the task starts and ends
    /// when the future returns.
    pub async fn wrap<T, F: Future<Output = T>>(&self, name: impl Into<String>, fut: F) -> T {
        let task = self.start(name.into());
        let result = fut.await;
        task.finish();
        result
    }
}
