use std::io::{IsTerminal, Write};
use std::sync::LazyLock;

use parking_lot::Mutex;

/// Cached check for whether stdout is a terminal.
static IS_TERMINAL: LazyLock<bool> = LazyLock::new(|| std::io::stdout().is_terminal());

/// Last emitted percentage, used to avoid redundant writes.
static LAST_PCT: Mutex<Option<u8>> = Mutex::new(None);

/// Context accumulated during a run to build the completion notification.
static SUMMARY: Mutex<NotifySummary> = Mutex::new(NotifySummary::new());

/// What happened during a run, used to phrase the completion notification.
///
/// `shown` gates the popup: it is only set once real work has happened (visible
/// progress was emitted or a package changed), so quick commands that did
/// nothing stay silent. The package lists let the notification say what
/// actually changed.
#[derive(Default)]
struct NotifySummary {
    shown: bool,
    installed: Vec<String>,
    removed: Vec<String>,
}

/// How many package names to list before collapsing the rest into a count.
const NAME_LIST_CAP: usize = 3;

impl NotifySummary {
    const fn new() -> Self {
        Self {
            shown: false,
            installed: Vec::new(),
            removed: Vec::new(),
        }
    }

    /// Phrase the notification text for the finished run.
    fn message(&self, success: bool) -> String {
        if !success {
            return "pixi: failed".to_owned();
        }

        let mut parts = Vec::new();
        if !self.installed.is_empty() {
            parts.push(format!("installed {}", join_names(&self.installed)));
        }
        if !self.removed.is_empty() {
            parts.push(format!("removed {}", join_names(&self.removed)));
        }

        match parts.is_empty() {
            true => "pixi: done".to_owned(),
            false => format!("pixi: {}", parts.join(", ")),
        }
    }
}

/// Join up to [`NAME_LIST_CAP`] names, collapsing any overflow into `+N more`.
fn join_names(names: &[String]) -> String {
    if names.len() <= NAME_LIST_CAP {
        return names.join(", ");
    }
    format!(
        "{}, +{} more",
        names[..NAME_LIST_CAP].join(", "),
        names.len() - NAME_LIST_CAP
    )
}

/// Record a package that was installed (or changed) during this run.
pub fn record_install(name: &str) {
    let mut summary = SUMMARY.lock();
    summary.shown = true;
    summary.installed.push(name.to_owned());
}

/// Record a package that was removed during this run.
pub fn record_removal(name: &str) {
    let mut summary = SUMMARY.lock();
    summary.shown = true;
    summary.removed.push(name.to_owned());
}

/// Emit an OSC 9;4 progress sequence to stdout.
///
/// Terminal emulators that support OSC 9;4 (Windows Terminal, iTerm2,
/// WezTerm, ConEmu, etc.) display this as progress in the title bar
/// or taskbar icon.
///
/// Uses ST (ESC \) as the string terminator for broad compatibility.
pub fn set_progress(position: u64, length: u64) {
    if !*IS_TERMINAL || length == 0 || crate::global_multi_progress().is_hidden() {
        return;
    }
    let pct = (position * 100 / length).min(100) as u8;
    let mut last = LAST_PCT.lock();
    if *last != Some(pct) {
        *last = Some(pct);
        SUMMARY.lock().shown = true;
        let seq = format!("\x1b]9;4;1;{pct}\x1b\\");
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(seq.as_bytes());
        let _ = stdout.flush();
    }
}

/// Clear the OSC 9;4 progress indicator.
pub fn clear_progress() {
    if !*IS_TERMINAL || crate::global_multi_progress().is_hidden() {
        return;
    }
    let mut last = LAST_PCT.lock();
    if last.is_some() {
        *last = None;
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(b"\x1b]9;4;0;0\x1b\\");
        let _ = stdout.flush();
    }
}

/// Show a desktop notification via OSC 9 once all operations have finished.
///
/// Terminal emulators that support OSC 9 (Windows Terminal, iTerm2, WezTerm,
/// etc.) surface this as a system popup, useful when the user has switched
/// away while a long operation runs. The message summarises which packages
/// were installed or removed, taken from the context recorded during the run.
///
/// Only emitted when real work was done during this run, so quick commands
/// that changed nothing stay silent. The summary is consumed so the popup
/// fires at most once per run.
pub fn notify_done(success: bool) {
    if !*IS_TERMINAL || crate::global_multi_progress().is_hidden() {
        return;
    }
    let summary = std::mem::take(&mut *SUMMARY.lock());
    if !summary.shown {
        return;
    }
    let seq = format!("\x1b]9;{}\x1b\\", summary.message(success));
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(seq.as_bytes());
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(installed: &[&str], removed: &[&str]) -> NotifySummary {
        NotifySummary {
            shown: true,
            installed: installed.iter().map(|s| (*s).to_owned()).collect(),
            removed: removed.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn message_lists_installed_only() {
        assert_eq!(
            summary(&["numpy", "pandas"], &[]).message(true),
            "pixi: installed numpy, pandas"
        );
    }

    #[test]
    fn message_lists_removed_only() {
        assert_eq!(
            summary(&[], &["scipy"]).message(true),
            "pixi: removed scipy"
        );
    }

    #[test]
    fn message_combines_installed_and_removed() {
        assert_eq!(
            summary(&["numpy"], &["scipy"]).message(true),
            "pixi: installed numpy, removed scipy"
        );
    }

    #[test]
    fn message_without_changes_is_done() {
        assert_eq!(summary(&[], &[]).message(true), "pixi: done");
    }

    #[test]
    fn message_on_failure_ignores_packages() {
        assert_eq!(summary(&["numpy"], &[]).message(false), "pixi: failed");
    }

    #[test]
    fn message_collapses_overflowing_names() {
        assert_eq!(
            summary(&["a", "b", "c", "d", "e"], &[]).message(true),
            "pixi: installed a, b, c, +2 more"
        );
    }

    #[test]
    fn join_names_lists_up_to_the_cap() {
        let names: Vec<String> = ["a", "b", "c"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(join_names(&names), "a, b, c");
    }

    #[test]
    fn join_names_collapses_past_the_cap() {
        let names: Vec<String> = ["a", "b", "c", "d"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(join_names(&names), "a, b, c, +1 more");
    }
}
