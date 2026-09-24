//! Helpers for returning executed command outcomes from `main`.
//!
//! On Unix, when a child is killed by a signal (e.g. SIGSEGV), the parent's
//! shell only prints messages like "Segmentation fault" if its own child
//! terminated via that signal. If pixi sits in between and exits "normally"
//! after waiting on the segfaulting grandchild, the message is lost and the
//! signal information is replaced by an arbitrary exit code. To preserve the
//! original behaviour we restore the default disposition for the signal and
//! re-raise it on ourselves.

use std::process::{ExitCode, ExitStatus};

/// Convert `status` to a portable code returned by the pixi process.
///
/// [`ExitCode`] only accepts values through `u8`, so larger platform-specific
/// child statuses are returned as [`ExitCode::FAILURE`].
pub fn exit_code_from_status(status: ExitStatus) -> ExitCode {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return exit_code_from_signal(signal);
        }
    }
    exit_code_from_code(status.code().unwrap_or(1))
}

/// Convert a numeric child exit code to the code returned by the pixi process.
///
/// Signal deaths may already be encoded as `128 + signal_number`, following
/// the POSIX shell convention used by `deno_task_shell`.
pub fn exit_code_from_code(code: i32) -> ExitCode {
    #[cfg(unix)]
    {
        if let Some(signal) = code.checked_sub(128).filter(|s| (1..=64).contains(s)) {
            return exit_code_from_signal(signal);
        }
    }
    numeric_exit_code(code)
}

fn numeric_exit_code(code: i32) -> ExitCode {
    u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from)
}

#[cfg(unix)]
fn exit_code_from_signal(signal: i32) -> ExitCode {
    // SAFETY: `signal(2)` and `raise(3)` are async-signal-safe and have no
    // Rust-level invariants to uphold.
    unsafe {
        libc::signal(signal as libc::c_int, libc::SIG_DFL);
        libc::raise(signal as libc::c_int);
    }
    // If raise() somehow returned (e.g. signal was blocked higher up), fall
    // back to the conventional encoded exit code.
    numeric_exit_code(128 + signal)
}
