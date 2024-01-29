use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressDrawTarget, ProgressState};
use once_cell::sync::Lazy;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt::Write;
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc::{channel, Sender};

/// Returns a global instance of [`indicatif::MultiProgress`].
///
/// Although you can always create an instance yourself any logging will interrupt pending
/// progressbars. To fix this issue, logging has been configured in such a way to it will not
/// interfere if you use the [`indicatif::MultiProgress`] returning by this function.
pub fn global_multi_progress() -> MultiProgress {
    static GLOBAL_MP: Lazy<MultiProgress> = Lazy::new(|| {
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

/// Returns the style to use for a progressbar that is in Deserializing state.
pub fn deserializing_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template("  {spinner:.dim} {prefix:20!} [{elapsed_precise}] {wide_msg}")
        .unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is finished.
pub fn finished_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template(&format!(
            "  {} {{prefix:20!}} [{{elapsed_precise}}] {{msg:.bold}}",
            console::style(console::Emoji("✔", " ")).green()
        ))
        .unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is in error state.
pub fn errored_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template(&format!(
            "  {} {{prefix:20!}} [{{elapsed_precise}}] {{msg:.bold.red}}",
            console::style(console::Emoji("❌", " ")).red()
        ))
        .unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is indeterminate and simply shows a spinner.
pub fn long_running_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::with_template("{spinner:.green} {msg}").unwrap()
}

// /// Displays a spinner with the given message while running the specified function to completion.
// pub fn wrap_in_progress<T, F: FnOnce() -> T>(msg: impl Into<Cow<'static, str>>, func: F) -> T {
//     let pb = global_multi_progress().add(ProgressBar::new_spinner());
//     pb.enable_steady_tick(Duration::from_millis(100));
//     pb.set_style(long_running_progress_style());
//     pb.set_message(msg);
//     let result = func();
//     pb.finish_and_clear();
//     result
// }

/// Displays a spinner with the given message while running the specified function to completion.
pub async fn await_in_progress<T, F: FnOnce(ProgressBar) -> Fut, Fut: Future<Output = T>>(
    msg: impl Into<Cow<'static, str>>,
    future: F,
) -> T {
    let pb = global_multi_progress().add(ProgressBar::new_spinner());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(long_running_progress_style());
    pb.set_message(msg);
    let result = future(pb.clone()).await;
    pb.finish_and_clear();
    result
}

/// A struct that can be used to format the message part of a progress bar.
///
/// It's primary usecase is when you have a single progress bar but multiple tasks that are running
/// and which you want to communicate to the user. This struct will set the message part of the
/// passed progress bar to the oldest unfinished task and include a the number of pending tasks.
#[derive(Debug, Clone)]
pub struct ProgressBarMessageFormatter {
    sender: Sender<Operation>,
    pb: ProgressBar,
}

enum Operation {
    Started(String),
    Finished(String),
}

pub struct ScopedTask {
    name: String,
    sender: Option<Sender<Operation>>,
    pb: ProgressBar,
}

impl ScopedTask {
    /// Finishes the execution of the task.
    pub async fn finish(mut self) -> ProgressBar {
        // Send the finished operation. If this fails the receiving end was most likely already
        // closed and we can just ignore the error.
        if let Some(sender) = self.sender.take() {
            let _ = sender
                .send(Operation::Finished(std::mem::take(&mut self.name)))
                .await;
        }
        self.pb.clone()
    }

    /// Returns the progress bar associated with the task
    pub fn progress_bar(&self) -> &ProgressBar {
        &self.pb
    }
}

impl ProgressBarMessageFormatter {
    /// Construct a new instance that will update the given progress bar.
    pub fn new(progress_bar: ProgressBar) -> Self {
        let pb = progress_bar.clone();
        let (tx, mut rx) = channel::<Operation>(20);
        tokio::spawn(async move {
            let mut pending = VecDeque::with_capacity(20);
            while let Some(msg) = rx.recv().await {
                match msg {
                    Operation::Started(op) => pending.push_back(op),
                    Operation::Finished(op) => {
                        let Some(idx) = pending.iter().position(|p| p == &op) else {
                            panic!("operation {op} was never started");
                        };
                        pending.remove(idx);
                    }
                }

                if pending.is_empty() {
                    progress_bar.set_message("");
                } else if pending.len() == 1 {
                    progress_bar.set_message(pending[0].clone());
                } else {
                    progress_bar.set_message(format!("{} (+{})", pending[0], pending.len() - 1));
                }
            }
        });
        Self { sender: tx, pb }
    }

    /// Returns the associated progress bar
    pub fn progress_bar(&self) -> &ProgressBar {
        &self.pb
    }

    /// Adds the start of another task to the progress bar and returns an object that is used to
    /// mark the lifetime of the task. If the object is dropped the task is considered finished.
    #[must_use]
    pub async fn start(&self, op: String) -> ScopedTask {
        self.sender
            .send(Operation::Started(op.clone()))
            .await
            .unwrap();
        ScopedTask {
            name: op,
            sender: Some(self.sender.clone()),
            pb: self.pb.clone(),
        }
    }

    /// Wraps an future into a task which starts when the task starts and ends when the future
    /// returns.
    pub async fn wrap<T, F: Future<Output = T>>(&self, name: impl Into<String>, fut: F) -> T {
        let task = self.start(name.into()).await;
        let result = fut.await;
        task.finish().await;
        result
    }

    /// Convert this instance into the underlying progress bar.
    pub fn into_progress_bar(self) -> ProgressBar {
        self.pb
    }
}
