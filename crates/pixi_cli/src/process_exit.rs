//! Helpers for exiting the pixi process so that the parent shell observes the
//! same outcome as the child command we executed.
//!
//! On Unix, when a child is killed by a signal (e.g. SIGSEGV), the parent's
//! shell only prints messages like "Segmentation fault" if its own child
//! terminated via that signal. If pixi sits in between and exits "normally"
//! after waiting on the segfaulting grandchild, the message is lost and the
//! signal information is replaced by an arbitrary exit code. To preserve the
//! original behaviour we restore the default disposition for the signal and
//! re-raise it on ourselves.

use std::process::ExitStatus;

/// Exit the current process mirroring how `status` terminated.
pub fn exit_with_status(status: ExitStatus) -> ! {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            exit_via_signal(signal);
        }
    }
    std::process::exit(status.code().unwrap_or(1));
}

/// Like [`exit_with_status`] but for callers that only have a numeric exit
/// code where signal deaths were already encoded as `128 + signal_number`
/// (the POSIX shell convention also used by `deno_task_shell`).
pub fn exit_with_code(code: i32) -> ! {
    #[cfg(unix)]
    {
        if let Some(signal) = code.checked_sub(128).filter(|s| (1..=64).contains(s)) {
            exit_via_signal(signal);
        }
    }
    std::process::exit(code);
}

#[cfg(unix)]
fn exit_via_signal(signal: i32) -> ! {
    // SAFETY: `signal(2)` and `raise(3)` are async-signal-safe and have no
    // Rust-level invariants to uphold.
    unsafe {
        libc::signal(signal as libc::c_int, libc::SIG_DFL);
        libc::raise(signal as libc::c_int);
    }
    // If raise() somehow returned (e.g. signal was blocked higher up), fall
    // back to the conventional encoded exit code.
    std::process::exit(128 + signal);
}
