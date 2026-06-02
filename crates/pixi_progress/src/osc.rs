use std::io::{IsTerminal, Write};
use std::sync::LazyLock;

use parking_lot::Mutex;

/// Cached check for whether stdout is a terminal.
static IS_TERMINAL: LazyLock<bool> = LazyLock::new(|| std::io::stdout().is_terminal());

/// Last emitted percentage, used to avoid redundant writes.
static LAST_PCT: Mutex<Option<u8>> = Mutex::new(None);

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
