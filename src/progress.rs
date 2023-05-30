use std::borrow::Cow;
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressDrawTarget, ProgressState};
use once_cell::sync::Lazy;
use std::fmt::Write;
use std::future::Future;
use std::time::Duration;

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
        .template("    {prefix:20!} [{elapsed_precise}] [{bar:40!.bright.yellow/dim.white}] {bytes:>8} @ {smoothed_bytes_per_sec:8}").unwrap()
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
        .template("    {prefix:20!} [{elapsed_precise}] [{bar:40!.bright.yellow/dim.white}] {pos:>7}/{len:7}").unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is in Deserializing state.
pub fn deserializing_progress_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::default_bar()
        .template("    {prefix:20!} [{elapsed_precise}] {wide_msg}")
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
pub async fn await_in_progress<T, F: Future<Output=T>>(msg: impl Into<Cow<'static, str>>, future: F) -> T {
    let pb = global_multi_progress().add(ProgressBar::new_spinner());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(long_running_progress_style());
    pb.set_message(msg);
    let result = future.await;
    pb.finish_and_clear();
    result
}
