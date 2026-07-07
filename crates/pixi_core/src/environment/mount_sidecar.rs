//! Mount sidecar lifecycle management.
//!
//! Manages the lifecycle of a daemonized mount process for a pixi environment.
//! Uses flock-based reference counting to coordinate between multiple pixi
//! processes sharing the same mounted environment.
//!
//! ## Protocol
//!
//! Per-environment coordination files (in the parent directory of the mount
//! point to avoid routing I/O through the NFS server):
//! - `{name}.rattler-fs.lock` — flock coordination file
//! - `{name}.rattler-fs.pid` — sidecar process PID
//!
//! ### Client side (pixi run/shell):
//! 1. Try `flock(LOCK_EX | LOCK_NB)` on lock file
//!    - Success + sidecar alive → reuse (grace period): downgrade to LOCK_SH
//!    - Success + sidecar dead → start sidecar, downgrade to LOCK_SH
//!    - EWOULDBLOCK → mount exists: acquire LOCK_SH, verify sidecar alive
//! 2. Run user's command
//! 3. On drop: release LOCK_SH (sidecar manages its own lifetime)
//!
//! ### Sidecar side:
//! 1. Mount, write PID, signal readiness
//! 2. Poll for client activity: try LOCK_EX on lock file every second
//!    - Success (no clients) → increment idle counter
//!    - Fail (clients active) → reset idle counter
//! 3. When idle >= grace period → unmount and exit

#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::io::{BufRead, BufReader};

use miette::{IntoDiagnostic, miette};
use rattler_conda_types::Platform;
use serde::{Deserialize, Serialize};

const LOCK_FILENAME: &str = ".rattler-fs.lock";
const PID_FILENAME: &str = ".rattler-fs.pid";
const OVERLAY_DIRNAME: &str = ".rattler-fs-overlay";
const LOG_FILENAME: &str = ".rattler-fs.log";

/// Derive the coordination file paths for a given mount point.
///
/// Both the lock file and PID file live in the parent directory of the mount
/// point, prefixed with the mount point's basename. This avoids routing I/O
/// through the NFS server that the sidecar hosts.
pub fn coordination_paths(mount_point: &Path) -> (PathBuf, PathBuf) {
    let parent = mount_point.parent().expect("mount_point has no parent");
    let basename = mount_point
        .file_name()
        .expect("mount_point has no file name")
        .to_string_lossy();
    (
        parent.join(format!("{basename}{LOCK_FILENAME}")),
        parent.join(format!("{basename}{PID_FILENAME}")),
    )
}

/// Path to the sidecar's log file (its captured stderr), beside the other
/// coordination files. The sidecar can outlive the spawning client's terminal,
/// so its diagnostics go here rather than to a possibly-dead inherited pty.
pub fn log_path(mount_point: &Path) -> PathBuf {
    let parent = mount_point.parent().expect("mount_point has no parent");
    let basename = mount_point
        .file_name()
        .expect("mount_point has no file name")
        .to_string_lossy();
    parent.join(format!("{basename}{LOG_FILENAME}"))
}

/// A `Stdio` for the sidecar's log file (truncated per session so it stays
/// bounded), falling back to null if it can't be opened.
fn sidecar_log_stdio(mount_point: &Path) -> std::process::Stdio {
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path(mount_point))
        .map(std::process::Stdio::from)
        .unwrap_or_else(|_| std::process::Stdio::null())
}

/// Read the tail of the sidecar log (up to `max_bytes`) to surface in an error
/// when the sidecar failed to become ready. Empty if the log is unreadable.
fn tail_log(mount_point: &Path, max_bytes: usize) -> String {
    match fs_err::read(log_path(mount_point)) {
        Ok(bytes) => {
            let start = bytes.len().saturating_sub(max_bytes);
            String::from_utf8_lossy(&bytes[start..]).trim().to_string()
        }
        Err(_) => String::new(),
    }
}

/// Persisted sidecar record — replaces the old bare-PID pidfile.
///
/// Written by the sidecar after it has mounted and before it signals
/// readiness, and removed only after it unmounts. The invariant the rest of
/// the protocol relies on is: *this file exists and validates ⟹ a live,
/// non-zombie sidecar owns a mount for exactly this
/// `(env_hash, transport, read_only)`* (mount health is additionally checked
/// on the reuse path via [`is_mounted`]).
///
/// `pid` alone is not enough — PIDs are recycled and pidfiles outlive reboots
/// — so `start_time` (seconds since the Unix epoch, via `sysinfo`) is stored
/// alongside it: a recycled PID has a different start time, and a stale
/// post-reboot pidfile matches no live process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarState {
    pub pid: u32,
    pub start_time: u64,
    /// Environment identity hash (see `rattler_fs::compute_env_hash`). Recorded
    /// for diagnostics.
    pub env_hash: String,
    /// Hash of the `pixi.lock` bytes the sidecar mounted from. The reuse check
    /// compares this against the client's current lock file, so a client whose
    /// lock file changed does not attach to a sidecar still serving the old
    /// environment. Hashing the file bytes (rather than recomputing an env hash
    /// with its own env-name/platform normalization) keeps the client and
    /// sidecar in exact agreement with zero drift.
    pub lock_hash: String,
    pub transport: String,
    pub read_only: bool,
    pub mount_point: PathBuf,
}

/// Hash the bytes of a `pixi.lock` file for the reuse-freshness check. Returns
/// an empty string if unreadable, which simply forces a conservative respawn.
/// Uses `DefaultHasher` (fixed seed → identical across the client and sidecar
/// processes); a non-cryptographic hash is sufficient for change detection.
fn hash_lock_file(lock_file_path: &Path) -> String {
    use std::hash::{Hash, Hasher};
    match fs_err::read(lock_file_path) {
        Ok(bytes) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        }
        Err(_) => String::new(),
    }
}

/// Query a process's start time (seconds since the Unix epoch), returning
/// `None` if the process does not exist or is a zombie. Cross-platform via
/// `sysinfo`, so there is a single liveness path on every OS.
fn live_process_start_time(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessStatus, ProcessesToUpdate, System};
    let mut sys = System::new();
    let spid = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[spid]), true);
    let process = sys.process(spid)?;
    if matches!(process.status(), ProcessStatus::Zombie) {
        return None;
    }
    Some(process.start_time())
}

/// Read and parse the sidecar record. Returns `None` if the file is missing
/// or is not a valid record — which includes the legacy bare-PID format, so an
/// old pidfile reads as "no valid sidecar" and is cleaned up as stale.
pub fn read_sidecar_state(pid_path: &Path) -> Option<SidecarState> {
    let bytes = fs_err::read(pid_path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Atomically write the sidecar record (write-tmp → rename) so readers never
/// observe a torn record.
fn write_sidecar_state(pid_path: &Path, state: &SidecarState) -> miette::Result<()> {
    let json = serde_json::to_vec_pretty(state).into_diagnostic()?;
    pixi_utils::atomic_write::atomic_write_sync(pid_path, &json).into_diagnostic()?;
    Ok(())
}

/// Build the record for the *current* process and write it atomically. Called
/// by the managed sidecar once its mount is live, before signaling readiness.
pub fn write_current_sidecar_state(
    pid_path: &Path,
    env_hash: &str,
    lock_file_path: &Path,
    transport: &str,
    read_only: bool,
    mount_point: &Path,
) -> miette::Result<()> {
    let pid = std::process::id();
    let start_time = live_process_start_time(pid).unwrap_or(0);
    let state = SidecarState {
        pid,
        start_time,
        env_hash: env_hash.to_string(),
        lock_hash: hash_lock_file(lock_file_path),
        transport: transport.to_string(),
        read_only,
        mount_point: mount_point.to_path_buf(),
    };
    write_sidecar_state(pid_path, &state)
}

/// RAII guard that holds a shared flock on the mount coordination file.
///
/// When dropped, releases the shared lock. The sidecar manages its own
/// lifetime via its polling loop and grace period.
pub struct MountGuard {
    #[allow(dead_code)]
    lock_file: File,
    #[allow(dead_code)]
    lock_path: PathBuf,
}

impl MountGuard {
    /// The path to the overlay directory for this environment.
    ///
    /// Lives in the parent directory of `env_dir` (not inside the mount point)
    /// to avoid routing writes through the NFS server.
    pub fn overlay_dir(env_dir: &Path) -> PathBuf {
        let parent = env_dir.parent().expect("env_dir has no parent");
        let name = env_dir
            .file_name()
            .expect("env_dir has no file name")
            .to_string_lossy();
        parent.join(format!("{name}{OVERLAY_DIRNAME}"))
    }
}

/// Whether a live, non-zombie sidecar owns the record at `pid_path`.
///
/// Cross-platform: reads the [`SidecarState`] record and requires both the PID
/// to exist and its start time to match, which defeats PID recycling, stale
/// post-reboot pidfiles, and zombies. This asserts *process* liveness, not
/// *mount* health — the reuse path additionally checks [`is_mounted`].
pub fn is_sidecar_alive(pid_path: &Path) -> bool {
    match read_sidecar_state(pid_path) {
        Some(state) => live_process_start_time(state.pid) == Some(state.start_time),
        None => false,
    }
}

/// Whether the sidecar recorded at `pid_path` can be reused by a client that
/// expects the given lock-file hash and read-only mode. Requires all of: a
/// live, non-zombie process (start time matches), a matching identity (the lock
/// file has not changed and the mode is the same — otherwise the sidecar serves
/// a stale environment), and an actually-live mount. This is the single reuse
/// predicate the coordination logic gates on.
fn is_reusable(env_dir: &Path, pid_path: &Path, lock_hash: &str, read_only: bool) -> bool {
    let Some(state) = read_sidecar_state(pid_path) else {
        return false;
    };
    live_process_start_time(state.pid) == Some(state.start_time)
        && state.lock_hash == lock_hash
        && state.read_only == read_only
        && is_mounted(env_dir)
}

// ─── Unix implementation ────────────────────────────────────────────────────

/// Check if a path is a mount point — **without stat'ing the mount itself**.
///
/// The old device-ID comparison called `fs::metadata(path)` on the mount
/// point, which is exactly the operation that misbehaves when the mount is
/// broken: a dead FUSE mount returns `ENOTCONN` (so the mount looked
/// *un*mounted and recovery/`pixi umount` silently no-op'd), and a dead NFS
/// hard-mount **blocks forever** (hanging `pixi run`/`pixi umount`). We instead
/// consult the kernel mount table, which lists dead mounts without touching
/// them. A path present in the table — even with a dead server — counts as
/// mounted, so callers treat it as "mounted, possibly broken" and force-unmount.
#[cfg(target_os = "linux")]
fn is_mountpoint(path: &Path) -> bool {
    let Ok(mountinfo) = fs_err::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    mountinfo_contains(&mountinfo, path)
}

/// Parse `/proc/self/mountinfo` and report whether `path` is a mount point.
/// Split out for unit testing without a real mount.
#[cfg(target_os = "linux")]
fn mountinfo_contains(mountinfo: &str, path: &Path) -> bool {
    let target = path.to_string_lossy();
    let target = target.trim_end_matches('/');
    for line in mountinfo.lines() {
        // Fields are space-separated; index 4 is the mount point, with
        // whitespace/control bytes octal-escaped (e.g. `\040` for space).
        if let Some(field) = line.split(' ').nth(4)
            && unescape_octal(field).trim_end_matches('/') == target
        {
            return true;
        }
    }
    false
}

/// Decode the `\NNN` octal escapes that `/proc/self/mountinfo` uses for space,
/// tab, newline and backslash in path fields.
#[cfg(target_os = "linux")]
fn unescape_octal(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..=i + 3]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
            && let Ok(code) = u8::from_str_radix(&s[i + 1..=i + 3], 8)
        {
            out.push(code as char);
            i += 4;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Check if a path is a mount point (macOS). Uses `getmntinfo(MNT_NOWAIT)`,
/// which reads the kernel mount table without touching the mounts, so it does
/// not hang on a dead NFS mount the way `stat` does. **Review-only:** written
/// for correctness parity with Linux but not exercised on the Linux CI here.
#[cfg(target_os = "macos")]
fn is_mountpoint(path: &Path) -> bool {
    use std::ffi::CStr;

    let mut mntbufp: *mut libc::statfs = std::ptr::null_mut();
    let count = unsafe { libc::getmntinfo(&mut mntbufp, libc::MNT_NOWAIT) };
    if count <= 0 {
        return false;
    }
    let target = path.to_string_lossy();
    let target = target.trim_end_matches('/');
    let mounts = unsafe { std::slice::from_raw_parts(mntbufp, count as usize) };
    mounts.iter().any(|m| {
        let mnt = unsafe { CStr::from_ptr(m.f_mntonname.as_ptr()) };
        mnt.to_string_lossy().trim_end_matches('/') == target
    })
}

/// Other Unix platforms don't support the mount backend; treat nothing as
/// mounted so the coordination code still compiles.
#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
fn is_mountpoint(_path: &Path) -> bool {
    false
}

/// Warn about — or refuse — reusing a writable mount's persistent overlay when
/// it was created for a different version of the environment (its recorded env
/// hash no longer matches `env_hash`, e.g. after `pixi add` changed the lock
/// file).
///
/// The overlay may hold files the user wants to keep (the results of
/// `pip install`), so the default is to reuse ("adopt") it. pixi owns this
/// user-facing message in every mount path — the client side of `ensure_mount`
/// for `pixi run` / `pixi shell` (the sidecar's own stderr goes to a log file),
/// and `pixi mount` directly for the interactive path — while the mount itself
/// honours the mapped rattler_fs policy as a backstop.
///
/// Returns an error only under [`pixi_config::OverlayMismatch::Error`]; `Warn`
/// prints a warning and `Ignore` is silent, both letting the mount proceed. A
/// missing overlay (no recorded hash) is "nothing to check". `environment_name`
/// and `env_hash` are used only for messaging and comparison, so callers pass
/// the same normalised name and hash the mount will use.
pub fn warn_or_error_on_overlay_mismatch(
    overlay_dir: &Path,
    environment_name: &str,
    env_hash: &str,
    overlay_mismatch: pixi_config::OverlayMismatch,
) -> miette::Result<()> {
    let Some(recorded) = rattler_fs::overlay::recorded_env_hash(overlay_dir) else {
        // No persistent overlay yet (read-only mount or first writable mount).
        return Ok(());
    };
    if recorded == env_hash {
        return Ok(());
    }

    match overlay_mismatch {
        pixi_config::OverlayMismatch::Error => Err(miette!(
            help = format!(
                "reuse it for the new environment by setting \
                 `mount-overlay-mismatch` to \"warn\" (the default) or \"ignore\", \
                 or discard it and rebuild with:\n\n    rm -rf {}",
                overlay_dir.display()
            ),
            "the mount overlay at {} was created for a different version of \
             environment '{}'. It may contain files you want to keep (for example \
             the results of `pip install`).",
            overlay_dir.display(),
            environment_name,
        )),
        pixi_config::OverlayMismatch::Warn => {
            tracing::warn!(
                "reusing the mount overlay at {} for a changed version of environment \
                 '{}'; files written to the previous version (e.g. `pip install`) are \
                 kept. Set `mount-overlay-mismatch = \"error\"` to refuse instead, or \
                 \"ignore\" to silence this warning.",
                overlay_dir.display(),
                environment_name,
            );
            Ok(())
        }
        pixi_config::OverlayMismatch::Ignore => Ok(()),
    }
}

/// Enforce the overlay-mismatch policy on the `ensure_mount` respawn path (while
/// we hold the exclusive lock and the mount is idle). A thin wrapper over
/// [`warn_or_error_on_overlay_mismatch`] that sources the env hash from disk,
/// since `ensure_mount` — unlike `pixi mount` — has not computed it.
///
/// A missing/unreadable overlay or lock file is treated as "nothing to check",
/// so any genuine problem surfaces later at mount time rather than here.
fn enforce_overlay_mismatch_policy(
    env_dir: &Path,
    lock_file_path: &Path,
    environment_name: &str,
    platform: Platform,
    overlay_mismatch: pixi_config::OverlayMismatch,
) -> miette::Result<()> {
    let overlay_dir = MountGuard::overlay_dir(env_dir);
    // Avoid parsing the lock file when there is no overlay to compare against.
    if rattler_fs::overlay::recorded_env_hash(&overlay_dir).is_none() {
        return Ok(());
    }

    // Compute the env hash the mount *will* use, matching the sidecar exactly:
    // an empty env name normalises to the default, same as `pixi mount`.
    let env_name = if environment_name.is_empty() {
        rattler_lock::DEFAULT_ENVIRONMENT_NAME
    } else {
        environment_name
    };
    let Ok(lock_file) = rattler_lock::LockFile::from_path(lock_file_path) else {
        return Ok(());
    };
    let Ok(env_hash) = rattler_fs::compute_env_hash(&lock_file, env_name, platform) else {
        return Ok(());
    };

    warn_or_error_on_overlay_mismatch(&overlay_dir, env_name, &env_hash, overlay_mismatch)
}

/// Ensure a mount is running for the given environment directory.
///
/// If no sidecar is running, starts one (by invoking `pixi mount --managed`).
/// If a sidecar is alive in its grace period, reuses it.
/// Returns a guard that keeps the shared flock alive.
#[cfg(unix)]
pub async fn ensure_mount(
    env_dir: &Path,
    workspace_root: &Path,
    environment_name: &str,
    lock_file_path: &Path,
    read_only: bool,
    platform: Platform,
    overlay_mismatch: pixi_config::OverlayMismatch,
) -> miette::Result<MountGuard> {
    fs_err::create_dir_all(env_dir).into_diagnostic()?;

    let (lock_path, pid_path) = coordination_paths(env_dir);

    // Identity the reuse check gates on: a sidecar serving a different lock file
    // (env changed) or a different mode must not be reused.
    let lock_hash = hash_lock_file(lock_file_path);

    let lock_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .into_diagnostic()?;

    let fd = lock_file.as_raw_fd();

    // Fast path: try a non-blocking exclusive lock. Success means no other
    // client holds the lock, so we are the only process permitted to mutate
    // mount state and may inspect/fix it under exclusivity.
    let got_ex = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0;

    if !got_ex {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(miette!("failed to acquire lock: {err}"));
        }

        // Another process holds the lock: either a starting client's exclusive
        // lock (we block until it downgrades) or other clients' shared locks.
        // Take a blocking shared lock.
        if unsafe { libc::flock(fd, libc::LOCK_SH) } != 0 {
            return Err(miette!(
                "failed to acquire shared lock: {}",
                std::io::Error::last_os_error()
            ));
        }

        // Reuse only if the sidecar is alive, its mount is actually live, and it
        // serves the same lock file and mode. A live PID with a dead mount
        // (zombie, dead NFS server, killed mount), or one serving a now-stale
        // lock file, must not be reused.
        if is_reusable(env_dir, &pid_path, &lock_hash, read_only) {
            return Ok(MountGuard {
                lock_file,
                lock_path,
            });
        }

        // Not reusable while other clients hold the lock. Two cases:
        //  - a *healthy* mount serving a different environment (e.g. the lock
        //    file changed under `pixi add`): it is in active use by those
        //    clients, so refuse rather than block indefinitely or yank it out
        //    from under them. The change is picked up once the mount is idle
        //    (the next use remounts it — see the exclusive path below).
        //  - a *broken* mount (dead sidecar / dead server): fall through to the
        //    blocking upgrade + respawn to recover it.
        if is_sidecar_alive(&pid_path) && is_mounted(env_dir) {
            return Err(miette!(
                "environment at {} is mounted from a different lock file and is in use by \
                 another pixi process. Close any active `pixi shell` / `pixi run` for it \
                 (or run `pixi umount`) and retry.",
                env_dir.display()
            ));
        }

        // The mount is broken but other clients still hold shared locks, so we
        // cannot clean up safely (that would force-unmount a mount a concurrent
        // client may have just started). Rather than hot-spin via recursion,
        // upgrade shared -> exclusive *blocking*: this waits for the stragglers
        // to drain instead of burning CPU. The upgrade is not atomic, so the
        // exclusive-held fix-up below re-checks state to cover the gap.
        if unsafe { libc::flock(fd, libc::LOCK_EX) } != 0 {
            return Err(miette!(
                "failed to upgrade to exclusive lock: {}",
                std::io::Error::last_os_error()
            ));
        }
    }

    // We hold the exclusive lock (fast path or blocking upgrade). We are the
    // sole process that may touch mount state. Reuse a healthy sidecar,
    // otherwise clean up the broken/absent mount and start a fresh one — all
    // under the lock (never in the unlocked window that let a stale cleanup
    // nuke a live successor mount).
    if is_reusable(env_dir, &pid_path, &lock_hash, read_only) {
        tracing::debug!("sidecar alive, mounted, and identity matches; reusing");
    } else {
        refuse_if_physical_install(env_dir)?;
        enforce_overlay_mismatch_policy(
            env_dir,
            lock_file_path,
            environment_name,
            platform,
            overlay_mismatch,
        )?;
        cleanup_stale_state(env_dir, &pid_path)?;
        start_sidecar(
            env_dir,
            workspace_root,
            environment_name,
            &lock_file,
            &pid_path,
        )
        .await?;
    }

    // Downgrade to a shared lock for the duration of the caller's work.
    if unsafe { libc::flock(fd, libc::LOCK_SH) } != 0 {
        return Err(miette!(
            "failed to downgrade to shared lock: {}",
            std::io::Error::last_os_error()
        ));
    }

    Ok(MountGuard {
        lock_file,
        lock_path,
    })
}

/// Start the sidecar mount process.
///
/// Creates a readiness pipe, forks the sidecar via `pixi mount --managed`,
/// and waits for it to signal readiness.
#[cfg(unix)]
async fn start_sidecar(
    env_dir: &Path,
    workspace_root: &Path,
    environment_name: &str,
    _lock_file: &File,
    pid_path: &Path,
) -> miette::Result<()> {
    // Create a pipe for readiness signaling, with both ends CLOEXEC so neither
    // leaks into the sidecar's helper subprocesses (a hung `sudo mount` /
    // `fusermount3` holding the write end would defer the readiness EOF and
    // stall us for the full timeout). The pre_exec below re-enables inheritance
    // of the write end for the sidecar itself, which re-sets CLOEXEC on it
    // before spawning any helper. Linux has atomic `pipe2`; elsewhere fall back
    // to `pipe` + `fcntl` (a small race window on other Unixes, acceptable).
    let mut pipe_fds = [0i32; 2];
    #[cfg(target_os = "linux")]
    let pipe_rc = unsafe { libc::pipe2(pipe_fds.as_mut_ptr(), libc::O_CLOEXEC) };
    #[cfg(not(target_os = "linux"))]
    let pipe_rc = {
        let rc = unsafe { libc::pipe(pipe_fds.as_mut_ptr()) };
        if rc == 0 {
            for &fd in &pipe_fds {
                unsafe {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags >= 0 {
                        libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                    }
                }
            }
        }
        rc
    };
    if pipe_rc != 0 {
        return Err(miette!(
            "failed to create pipe: {}",
            std::io::Error::last_os_error()
        ));
    }
    let (read_fd, write_fd) = (pipe_fds[0], pipe_fds[1]);

    // Find the pixi binary
    let pixi_exe = std::env::current_exe().into_diagnostic()?;

    let env_dir_str = env_dir.display().to_string();
    let write_fd_str = write_fd.to_string();
    let pid_path_str = pid_path.display().to_string();

    // Spawn the sidecar as a detached child process.
    let mut cmd = std::process::Command::new(&pixi_exe);
    cmd.args([
        "mount",
        "--managed",
        "-e",
        environment_name,
        "--mount-point",
        &env_dir_str,
        "--pidfile",
        &pid_path_str,
        "--ready-fd",
        &write_fd_str,
    ])
    .current_dir(workspace_root)
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(sidecar_log_stdio(env_dir));

    // Detach the sidecar into its own session, and let it inherit the
    // readiness pipe's write end.
    unsafe {
        use std::os::unix::process::CommandExt;
        let fd = write_fd;
        cmd.pre_exec(move || {
            // setsid() makes the sidecar a session leader with no controlling
            // terminal, so a Ctrl-C (SIGINT) or terminal close (SIGHUP) on the
            // spawning client's terminal no longer reaches it. Without this, one
            // client's Ctrl-C tears down a mount other clients are still using
            // (and SIGHUP, which the sidecar does not handle, killed it with no
            // unmount at all). setsid() succeeds because the freshly forked
            // child is never already a process-group leader.
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Clear CLOEXEC on the readiness pipe's write end so the child keeps
            // it across exec.
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd.spawn().into_diagnostic()?;

    // Close the write end in the parent
    unsafe {
        libc::close(write_fd);
    }

    // Wait for readiness signal from the sidecar
    let read_file = unsafe { File::from_raw_fd(read_fd) };
    let mut reader = BufReader::new(read_file);
    let mut line = String::new();

    // Use a timeout to avoid blocking forever if the sidecar fails
    let readiness = tokio::task::spawn_blocking(move || reader.read_line(&mut line).map(|_| line));

    match tokio::time::timeout(std::time::Duration::from_secs(30), readiness).await {
        Ok(Ok(Ok(msg))) if msg.starts_with("ready") => {
            tracing::debug!("mount sidecar is ready");
            Ok(())
        }
        Ok(Ok(Ok(msg))) => {
            // The sidecar exited before signaling ready (pipe closed → empty
            // read) or reported an error. It is likely already gone, but kill it
            // for good measure so we never leak a half-started sidecar.
            let _ = child.kill();
            let msg = msg.trim();
            let detail = if msg.is_empty() {
                tail_log(env_dir, 4096)
            } else {
                msg.to_string()
            };
            Err(miette!("mount sidecar failed to start: {detail}"))
        }
        Ok(Ok(Err(e))) => {
            let _ = child.kill();
            Err(miette!("failed to read from sidecar pipe: {e}"))
        }
        Ok(Err(e)) => {
            let _ = child.kill();
            Err(miette!("sidecar readiness task failed: {e}"))
        }
        Err(_) => {
            // Timeout — kill the child if still running
            let _ = child.kill();
            let log = tail_log(env_dir, 4096);
            let hint = if log.is_empty() {
                String::new()
            } else {
                format!("\nsidecar log:\n{log}")
            };
            Err(miette!(
                "timed out waiting for mount sidecar to become ready{hint}"
            ))
        }
    }
}

// ─── Windows implementation ─────────────────────────────────────────────────

/// Check if a path is an active ProjFS virtualization root.
///
/// ProjFS doesn't create mount points. Instead we check if the sidecar PID
/// file exists and the sidecar is alive.
#[cfg(windows)]
fn is_mountpoint(path: &Path) -> bool {
    let (_, pid_path) = coordination_paths(path);
    is_sidecar_alive(&pid_path)
}

/// Ensure a mount is running for the given environment directory (Windows).
///
/// Uses `LockFileEx` for coordination instead of `flock`.
#[cfg(windows)]
pub async fn ensure_mount(
    env_dir: &Path,
    workspace_root: &Path,
    environment_name: &str,
    lock_file_path: &Path,
    read_only: bool,
    platform: Platform,
    overlay_mismatch: pixi_config::OverlayMismatch,
) -> miette::Result<MountGuard> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx, UnlockFileEx,
    };

    fs_err::create_dir_all(env_dir).into_diagnostic()?;

    let (lock_path, pid_path) = coordination_paths(env_dir);
    let lock_hash = hash_lock_file(lock_file_path);

    let lock_file = OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .truncate(false)
        .open(&lock_path)
        .into_diagnostic()?;

    let handle = lock_file.as_raw_handle();
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };

    // Try exclusive lock (non-blocking)
    let got_exclusive = unsafe {
        LockFileEx(
            handle,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        ) != 0
    };

    if got_exclusive {
        // We got exclusive lock — no other clients.
        if is_reusable(env_dir, &pid_path, &lock_hash, read_only) {
            tracing::debug!("sidecar alive, mounted, and identity matches; reusing");
        } else {
            refuse_if_physical_install(env_dir)?;
            cleanup_stale_state(env_dir, &pid_path)?;
            start_sidecar(
                env_dir,
                workspace_root,
                environment_name,
                &lock_file,
                &pid_path,
            )
            .await?;
        }

        // Release exclusive lock, then acquire shared lock
        unsafe {
            overlapped = std::mem::zeroed();
            UnlockFileEx(handle, 0, 1, 0, &mut overlapped);
            // Acquire shared (non-exclusive) lock — blocking
            overlapped = std::mem::zeroed();
            if LockFileEx(handle, 0, 0, 1, 0, &mut overlapped) == 0 {
                return Err(miette!(
                    "failed to acquire shared lock: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
            // Mount is starting or running. Acquire shared lock (blocking).
            unsafe {
                if LockFileEx(handle, 0, 0, 1, 0, &mut overlapped) == 0 {
                    return Err(miette!(
                        "failed to acquire shared lock: {}",
                        std::io::Error::last_os_error()
                    ));
                }
            }

            // Verify the sidecar is reusable (alive, mounted, matching identity)
            if !is_reusable(env_dir, &pid_path, &lock_hash, read_only) {
                // A healthy mount serving a different environment, in use by
                // other clients: refuse rather than tear it out from under them.
                if is_sidecar_alive(&pid_path) && is_mounted(env_dir) {
                    return Err(miette!(
                        "environment at {} is mounted from a different lock file and is in \
                         use by another pixi process. Close any active `pixi shell` / \
                         `pixi run` for it (or run `pixi umount`) and retry.",
                        env_dir.display()
                    ));
                }
                unsafe {
                    overlapped = std::mem::zeroed();
                    UnlockFileEx(handle, 0, 1, 0, &mut overlapped);
                }
                drop(lock_file);
                cleanup_stale_state(env_dir, &pid_path)?;
                return Box::pin(ensure_mount(
                    env_dir,
                    workspace_root,
                    environment_name,
                    lock_file_path,
                    read_only,
                    platform,
                    overlay_mismatch,
                ))
                .await;
            }
        } else {
            return Err(miette!("failed to acquire lock: {err}"));
        }
    }

    Ok(MountGuard {
        lock_file,
        lock_path,
    })
}

/// Start the sidecar mount process (Windows).
///
/// Uses a named event for readiness signaling instead of a pipe/fd.
#[cfg(windows)]
async fn start_sidecar(
    env_dir: &Path,
    workspace_root: &Path,
    environment_name: &str,
    _lock_file: &File,
    pid_path: &Path,
) -> miette::Result<()> {
    // Use a unique named event for readiness signaling.
    // Include a hash of env_dir to prevent collisions when multiple
    // environments are mounted concurrently or PIDs are recycled.
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    env_dir.hash(&mut hasher);
    let event_name = format!(
        "Local\\pixi-mount-ready-{}-{:x}",
        std::process::id(),
        hasher.finish()
    );

    // Create a named event
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    let event_name_wide: Vec<u16> = OsStr::new(&event_name)
        .encode_wide()
        .chain(Some(0))
        .collect();

    let event_handle = unsafe {
        windows_sys::Win32::System::Threading::CreateEventW(
            std::ptr::null(),
            1, // manual reset
            0, // initial state: not signaled
            event_name_wide.as_ptr(),
        )
    };
    if event_handle.is_null() {
        return Err(miette!(
            "failed to create readiness event: {}",
            std::io::Error::last_os_error()
        ));
    }

    let pixi_exe = std::env::current_exe().into_diagnostic()?;
    let env_dir_str = env_dir.display().to_string();
    let pid_path_str = pid_path.display().to_string();

    let mut cmd = std::process::Command::new(&pixi_exe);
    cmd.args([
        "mount",
        "--managed",
        "-e",
        environment_name,
        "--mount-point",
        &env_dir_str,
        "--pidfile",
        &pid_path_str,
        "--ready-event",
        &event_name,
    ])
    .current_dir(workspace_root)
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(sidecar_log_stdio(env_dir));

    // Detach the sidecar from the spawning client's console and process group,
    // the Windows analogue of setsid() on Unix: a Ctrl-C or console close on the
    // parent must not tear down a mount other clients still use. Review-only:
    // not exercised on the Linux CI here.
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    let mut child = cmd.spawn().into_diagnostic()?;

    // Wait for readiness event with timeout.
    // Cast to isize for Send safety — Windows HANDLEs are just kernel object
    // pointers and are safe to use from any thread.
    let event_handle_isize = event_handle as isize;
    let wait_result = tokio::task::spawn_blocking(move || unsafe {
        let h = event_handle_isize as windows_sys::Win32::Foundation::HANDLE;
        let result = windows_sys::Win32::System::Threading::WaitForSingleObject(h, 30_000);
        windows_sys::Win32::Foundation::CloseHandle(h);
        result
    })
    .await
    .into_diagnostic()?;

    use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT};

    match wait_result {
        WAIT_OBJECT_0 => {
            tracing::debug!("mount sidecar is ready");
            Ok(())
        }
        WAIT_TIMEOUT => {
            let _ = child.kill();
            Err(miette!(
                "timed out waiting for mount sidecar to become ready"
            ))
        }
        WAIT_ABANDONED => {
            let _ = child.kill();
            Err(miette!(
                "sidecar readiness event was abandoned (sidecar may have crashed)"
            ))
        }
        other => {
            let _ = child.kill();
            Err(miette!(
                "WaitForSingleObject returned unexpected status: 0x{:08x}",
                other
            ))
        }
    }
}

// ─── Platform-independent code ──────────────────────────────────────────────

/// Best-effort terminate a process by PID — SIGTERM (or SIGKILL when `force`)
/// on Unix, `TerminateProcess` on Windows. Callers MUST have already confirmed
/// the PID's start time matches the recorded sidecar, so this never kills a
/// recycled PID.
fn terminate_process(pid: u32, force: bool) {
    #[cfg(unix)]
    unsafe {
        let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
        libc::kill(pid as libc::pid_t, sig);
    }
    #[cfg(windows)]
    unsafe {
        let _ = force;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

/// Clean up stale state left by a previous sidecar, so a fresh mount can start.
///
/// MUST be called while holding the exclusive coordination lock: it
/// force-unmounts and terminates processes, which would race a concurrent
/// client's fresh mount if done unlocked.
fn cleanup_stale_state(env_dir: &Path, pid_path: &Path) -> miette::Result<()> {
    // If a sidecar process is still alive at this point, terminate it — the
    // ensure-mount recovery path reaches here only for an unhealthy mount, and
    // `pixi umount` reaches here to stop a healthy one. Terminating it (which is
    // start-time-verified, so never a recycled PID) makes it unmount and release
    // the overlay lock; otherwise a respawn would block on that lock and hit the
    // readiness timeout. Bounded SIGTERM → SIGKILL escalation.
    if let Some(state) = read_sidecar_state(pid_path)
        && live_process_start_time(state.pid) == Some(state.start_time)
    {
        tracing::debug!("terminating unhealthy sidecar pid {}", state.pid);
        terminate_process(state.pid, false);
        let mut exited = false;
        for i in 0..30 {
            if live_process_start_time(state.pid) != Some(state.start_time) {
                exited = true;
                break;
            }
            if i == 20 {
                // Ignored SIGTERM for ~2s — escalate.
                terminate_process(state.pid, true);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !exited {
            tracing::warn!("sidecar pid {} did not exit after SIGKILL", state.pid);
        }
    }

    // Remove the (now stale) pidfile and log file.
    if pid_path.exists() {
        tracing::debug!("cleaning up stale pidfile at {}", pid_path.display());
        let _ = fs_err::remove_file(pid_path);
    }
    let _ = fs_err::remove_file(log_path(env_dir));

    // Force-unmount any mount the dead/killed sidecar left behind.
    if is_mountpoint(env_dir) {
        tracing::debug!("force-unmounting stale mount at {}", env_dir.display());
        force_unmount(env_dir)?;
    }

    Ok(())
}

/// Map the transport name recorded in the sidecar state back to a
/// [`rattler_fs::Transport`].
fn transport_from_name(name: &str) -> rattler_fs::Transport {
    match name {
        "fuse" => rattler_fs::Transport::Fuse,
        "nfs" => rattler_fs::Transport::Nfs,
        "projfs" => rattler_fs::Transport::ProjFs,
        _ => rattler_fs::Transport::Auto,
    }
}

/// Force-unmount a mount point using the transport the sidecar recorded
/// (falling back to the platform default). Best-effort backstop for a stale
/// mount left by a crashed or killed sidecar.
///
/// Delegates to the transport-aware [`rattler_fs::force_unmount`] instead of the
/// old FUSE-only `fusermount3` shell-out, which silently failed on Linux NFS
/// mounts. On Windows, ProjFS virtualization stops when the owning sidecar
/// process exits, so terminating the sidecar (the caller's job) is the teardown;
/// there is nothing to unmount here.
pub fn force_unmount(mount_point: &Path) -> miette::Result<()> {
    let (_, pid_path) = coordination_paths(mount_point);
    let transport = read_sidecar_state(&pid_path)
        .map(|s| transport_from_name(&s.transport))
        .unwrap_or(rattler_fs::Transport::Auto);

    #[cfg(unix)]
    {
        rattler_fs::force_unmount(mount_point, transport)
            .map_err(|e| miette!("failed to force-unmount {}: {e}", mount_point.display()))
    }
    #[cfg(windows)]
    {
        let _ = transport;
        Ok(())
    }
}

/// Check if a mount is currently active for the given environment.
pub fn is_mounted(env_dir: &Path) -> bool {
    is_mountpoint(env_dir)
}

/// Whether `env_dir` holds a physical (link-backend) conda install rather than
/// being empty and ready to mount. A live or stale mount is *not* a physical
/// install — those are handled by cleanup — so this is false while mounted; it
/// only detects a real on-disk install that a mount would silently shadow.
fn has_physical_install(env_dir: &Path) -> bool {
    !is_mounted(env_dir) && env_dir.join("conda-meta").exists()
}

/// Refuse to mount over a pre-existing link-backend install: the mount would
/// hide it (and it would reappear on unmount), wasting space and confusing
/// anything that inspects the directory. Point the user at removing it.
fn refuse_if_physical_install(env_dir: &Path) -> miette::Result<()> {
    if has_physical_install(env_dir) {
        return Err(miette!(
            "environment at {} already has a link-backend (physical) install, which \
             the mount would silently hide. Remove it first (e.g. `rm -rf {}`), or keep \
             `experimental.environment-backend = \"link\"` for this environment.",
            env_dir.display(),
            env_dir.display()
        ));
    }
    Ok(())
}

/// Whether another pixi process currently holds the coordination lock for this
/// mount (i.e. a `pixi run`/`pixi shell` is actively using it). Used by
/// `pixi umount` to refuse a disruptive unmount unless `--force` is given.
///
/// This is a point-in-time probe: it can momentarily read `true` while the
/// sidecar holds its per-second probe lock, so treat it as advisory.
pub fn clients_attached(mount_point: &Path) -> bool {
    let (lock_path, _) = coordination_paths(mount_point);
    let Ok(file) = OpenOptions::new().read(true).write(true).open(&lock_path) else {
        return false; // no lock file → nothing attached
    };
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            unsafe { libc::flock(fd, libc::LOCK_UN) };
            false
        } else {
            true
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx, UnlockFileEx,
        };
        let handle = file.as_raw_handle();
        let mut ov: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
        if unsafe {
            LockFileEx(
                handle,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                1,
                0,
                &mut ov,
            ) != 0
        } {
            unsafe { UnlockFileEx(handle, 0, 1, 0, &mut ov) };
            false
        } else {
            true
        }
    }
}

/// Tear down the sidecar and mount at `mount_point` for `pixi umount`:
/// terminate the sidecar (start-time-verified, SIGTERM → SIGKILL), remove the
/// coordination record, and force-unmount any residue. Returns whether anything
/// was actually torn down (so the caller can report accurately). Shares the
/// implementation with stale-state recovery.
pub fn teardown_mount(mount_point: &Path) -> miette::Result<bool> {
    let (_, pid_path) = coordination_paths(mount_point);
    let was_active = is_sidecar_alive(&pid_path) || is_mounted(mount_point);
    cleanup_stale_state(mount_point, &pid_path)?;
    Ok(was_active)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err as fs;
    use tempfile::TempDir;

    #[test]
    fn test_coordination_paths() {
        let mount_point = PathBuf::from("/tmp/envs/default");
        let (lock_path, pid_path) = coordination_paths(&mount_point);
        assert_eq!(
            lock_path,
            PathBuf::from("/tmp/envs/default.rattler-fs.lock")
        );
        assert_eq!(pid_path, PathBuf::from("/tmp/envs/default.rattler-fs.pid"));
    }

    #[test]
    fn test_is_sidecar_alive_nonexistent_pid() {
        let tmp = TempDir::new().unwrap();
        let pid_path = tmp.path().join("test.pid");
        assert!(!is_sidecar_alive(&pid_path));
    }

    #[test]
    fn test_is_sidecar_alive_invalid_pid() {
        let tmp = TempDir::new().unwrap();
        let pid_path = tmp.path().join("test.pid");
        fs::write(&pid_path, "not_a_number\n").unwrap();
        assert!(!is_sidecar_alive(&pid_path));
    }

    fn fake_state(pid: u32, start_time: u64) -> SidecarState {
        SidecarState {
            pid,
            start_time,
            env_hash: "sha256:test".to_string(),
            lock_hash: "deadbeef".to_string(),
            transport: "fuse".to_string(),
            read_only: true,
            mount_point: PathBuf::from("/tmp/env"),
        }
    }

    #[test]
    fn test_hash_lock_file_changes_with_content() {
        let tmp = TempDir::new().unwrap();
        let lock = tmp.path().join("pixi.lock");
        fs::write(&lock, b"version: 6\npackages: []\n").unwrap();
        let h1 = hash_lock_file(&lock);
        assert!(!h1.is_empty());
        // Same content → same hash (client and sidecar agree).
        assert_eq!(h1, hash_lock_file(&lock));
        // Changed content → different hash (reuse is invalidated).
        fs::write(&lock, b"version: 6\npackages: [changed]\n").unwrap();
        assert_ne!(h1, hash_lock_file(&lock));
        // Missing file → empty (forces a conservative respawn).
        assert_eq!(hash_lock_file(&tmp.path().join("absent.lock")), "");
    }

    #[test]
    fn test_is_sidecar_alive_dead_pid() {
        let tmp = TempDir::new().unwrap();
        let pid_path = tmp.path().join("test.pid");
        // A valid record whose PID (almost certainly) does not exist.
        write_sidecar_state(&pid_path, &fake_state(99_999_999, 1)).unwrap();
        assert!(!is_sidecar_alive(&pid_path));
    }

    #[test]
    fn test_is_sidecar_alive_current_process() {
        let tmp = TempDir::new().unwrap();
        let pid_path = tmp.path().join("test.pid");
        // Records the current process with its real start time.
        write_current_sidecar_state(&pid_path, "sha256:h", tmp.path(), "fuse", true, tmp.path())
            .unwrap();
        assert!(is_sidecar_alive(&pid_path));
    }

    #[test]
    fn test_is_sidecar_alive_rejects_recycled_pid() {
        // Same (live) PID but a start time that does not match must NOT count as
        // alive — this is the PID-recycling / stale-post-reboot defense (#6).
        let tmp = TempDir::new().unwrap();
        let pid_path = tmp.path().join("test.pid");
        let live_pid = std::process::id();
        let real_start = live_process_start_time(live_pid).expect("own process is live");
        write_sidecar_state(&pid_path, &fake_state(live_pid, real_start.wrapping_add(1))).unwrap();
        assert!(!is_sidecar_alive(&pid_path));
    }

    #[test]
    fn test_legacy_bare_pid_is_treated_as_stale() {
        // An old bare-PID pidfile (pre-record format) is not valid JSON, so it
        // reads as "no sidecar" and gets cleaned up rather than trusted.
        let tmp = TempDir::new().unwrap();
        let pid_path = tmp.path().join("test.pid");
        let pid = std::process::id();
        fs::write(&pid_path, format!("{pid}\n")).unwrap();
        assert!(read_sidecar_state(&pid_path).is_none());
        assert!(!is_sidecar_alive(&pid_path));
    }

    #[test]
    fn test_cleanup_stale_state_removes_dead_pidfile() {
        let tmp = TempDir::new().unwrap();
        let env_dir = tmp.path().join("env");
        fs::create_dir_all(&env_dir).unwrap();
        let pid_path = env_dir.join(PID_FILENAME);
        fs::write(&pid_path, "99999999\n").unwrap();

        cleanup_stale_state(&env_dir, &pid_path).unwrap();
        assert!(!pid_path.exists());
    }

    #[test]
    fn test_overlay_dir() {
        let env_dir = PathBuf::from("/tmp/test-env");
        assert_eq!(
            MountGuard::overlay_dir(&env_dir),
            PathBuf::from("/tmp/test-env.rattler-fs-overlay")
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_is_mountpoint_regular_dir() {
        let tmp = TempDir::new().unwrap();
        // A regular directory is not a mount point.
        assert!(!is_mountpoint(tmp.path()));
    }

    #[cfg(unix)]
    #[test]
    fn test_has_physical_install() {
        let tmp = TempDir::new().unwrap();
        let env = tmp.path().join("env");
        fs::create_dir_all(&env).unwrap();
        // Empty (fresh) env dir → not a physical install → mounting is allowed.
        assert!(!has_physical_install(&env));
        assert!(refuse_if_physical_install(&env).is_ok());
        // A conda-meta directory marks a physical (link-backend) install.
        fs::create_dir_all(env.join("conda-meta")).unwrap();
        assert!(has_physical_install(&env));
        assert!(refuse_if_physical_install(&env).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_mountinfo_contains() {
        // Two real-ish mountinfo lines; index 4 is the mount point, and one of
        // them uses the `\040` octal escape for a space.
        let sample = "\
36 35 98:0 / / rw,relatime shared:1 - ext4 /dev/root rw
41 36 0:33 / /home/user/my\\040env rw,relatime shared:2 - fuse conda-packages rw
42 36 0:34 / /mnt/other rw - nfs server:/ rw
";
        assert!(mountinfo_contains(sample, Path::new("/")));
        assert!(mountinfo_contains(
            sample,
            Path::new("/home/user/my env") // unescaped space
        ));
        assert!(mountinfo_contains(sample, Path::new("/mnt/other/"))); // trailing slash tolerated
        assert!(!mountinfo_contains(sample, Path::new("/not/mounted")));
    }
}
