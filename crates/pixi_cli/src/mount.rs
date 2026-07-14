use std::path::PathBuf;

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_config::{Config, ConfigCli};
use pixi_core::{
    UpdateLockFileOptions, WorkspaceLocator,
    environment::get_update_lock_file_and_prefix,
    lock_file::{ReinstallPackages, UpdateMode},
};
use rattler::package_cache::PackageCache;
use rattler_conda_types::Platform;
use rattler_lock::DEFAULT_ENVIRONMENT_NAME;

use crate::cli_config::{LockAndInstallConfig, WorkspaceConfig};

/// Mount a pixi environment as a virtual filesystem.
///
/// In interactive mode (default), the environment is mounted and the command
/// blocks until Ctrl+C is pressed. In managed mode (--managed), the process
/// daemonizes and waits for SIGTERM — this is used internally by `pixi run`
/// and `pixi shell` when `experimental.environment-backend = "mount"`.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    lock_and_install_config: LockAndInstallConfig,

    #[clap(flatten)]
    config: ConfigCli,

    /// The environment to mount.
    #[arg(long, short)]
    environment: Option<String>,

    /// Mount point override. Defaults to the environment directory.
    #[arg(long)]
    mount_point: Option<PathBuf>,

    // --- Hidden sidecar flags (used by pixi run/shell internally) ---
    /// Run as a managed sidecar process.
    #[arg(long, hide = true)]
    managed: bool,

    /// Path to the pidfile (used with --managed).
    #[arg(long, hide = true)]
    pidfile: Option<PathBuf>,

    /// File descriptor number for the readiness pipe (used with --managed, Unix only).
    #[arg(long, hide = true)]
    ready_fd: Option<i32>,

    /// Named event for readiness signaling (used with --managed, Windows only).
    #[arg(long, hide = true)]
    ready_event: Option<String>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::from(args.config.clone());
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(config);

    let environment = workspace.environment_from_name_or_env_var(args.environment)?;
    let env_dir = environment.dir();
    let mount_point = args.mount_point.unwrap_or_else(|| env_dir.clone());
    let platform = environment
        .best_declared_platform()
        .map(|p| p.subdir())
        .unwrap_or_else(Platform::current);

    let env_name = environment.name().as_str();
    let env_name = if env_name.is_empty() {
        DEFAULT_ENVIRONMENT_NAME
    } else {
        env_name
    };

    // Determine overlay based on read-only config
    let overlay_dir = if workspace.config().mount_read_only() {
        None
    } else {
        Some(pixi_core::environment::mount_sidecar::MountGuard::overlay_dir(&env_dir))
    };

    // The user-facing warning for an adopted overlay is printed client-side by
    // `ensure_mount` (the sidecar's stderr goes to a log file), so here `warn`
    // and `ignore` both map to "adopt"; only `error` refuses inside rattler_fs.
    let overlay_mismatch = map_overlay_mismatch(workspace.config().mount_overlay_mismatch());

    let transport = match workspace.config().mount_backend() {
        pixi_config::MountBackend::Auto => rattler_fs::Transport::Auto,
        pixi_config::MountBackend::Nfs => rattler_fs::Transport::Nfs,
        pixi_config::MountBackend::Fuse => rattler_fs::Transport::Fuse,
    };

    // Reject an unavailable transport early with an actionable message instead
    // of failing deep inside the mount (e.g. `fuse` on macOS needs rattler_fs
    // built with the `fuse` feature / macFUSE; NFS is the macOS default).
    if !transport.is_available() {
        return Err(miette::miette!(
            "mount transport {transport:?} is not available on this platform/build. \
             On macOS the FUSE backend requires macFUSE and rattler_fs built with the \
             `fuse` feature; use the default `mount-backend = \"nfs\"` instead."
        ));
    }

    if args.managed {
        // The managed sidecar is spawned by ensure_mount after the parent has
        // already solved and cached packages. Just load the existing lock file
        // and package cache — no solve needed.
        let lock_file_path = workspace.lock_file_path();
        let lock_file = rattler_lock::LockFile::from_path(&lock_file_path).into_diagnostic()?;
        let package_cache = PackageCache::new(
            pixi_config::get_cache_dir()?.join(pixi_consts::consts::CONDA_PACKAGE_CACHE_DIR),
        );

        let env_hash = rattler_fs::compute_env_hash(&lock_file, env_name, platform)
            .map_err(|e| miette::miette!("failed to compute env hash: {e}"))?;

        let grace_period = workspace.config().mount_grace_period();

        execute_managed(
            &lock_file,
            env_name,
            platform,
            &package_cache,
            &mount_point,
            overlay_dir,
            overlay_mismatch,
            &env_hash,
            &lock_file_path,
            transport,
            grace_period,
            args.pidfile.as_deref(),
            args.ready_fd,
            args.ready_event.as_deref(),
        )
        .await
    } else {
        // Interactive mode: ensure lock file is up-to-date and packages are
        // cached (but skip the hardlink install — the mount replaces it).
        let (lock_file_data, _prefix) = get_update_lock_file_and_prefix(
            &environment,
            None,
            UpdateMode::Revalidate,
            UpdateLockFileOptions {
                lock_file_usage: args.lock_and_install_config.lock_file_usage()?,
                no_install: true,
                upgrade_lock_file_format: false,
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
            },
            ReinstallPackages::default(),
            &pixi_core::environment::InstallFilter::default(),
        )
        .await?;

        let env_hash = rattler_fs::compute_env_hash(&lock_file_data.lock_file, env_name, platform)
            .map_err(|e| miette::miette!("failed to compute env hash: {e}"))?;

        // Interactive mounts don't go through `ensure_mount`, so do the
        // overlay-mismatch check (warn / error naming the config option) here,
        // before mounting. The managed path is handled by `ensure_mount`.
        if let Some(overlay_dir) = &overlay_dir {
            pixi_core::environment::mount_sidecar::warn_or_error_on_overlay_mismatch(
                overlay_dir,
                env_name,
                &env_hash,
                workspace.config().mount_overlay_mismatch(),
            )?;
        }

        execute_interactive(
            &lock_file_data.lock_file,
            env_name,
            platform,
            &lock_file_data.package_cache,
            &mount_point,
            overlay_dir,
            overlay_mismatch,
            &env_hash,
            transport,
        )
        .await
    }
}

/// Map the pixi overlay-mismatch config to the rattler_fs mount policy. Both
/// `warn` and `ignore` adopt the overlay (rattler_fs has no separate "warn"
/// state — pixi owns the user-facing message); only `error` refuses.
fn map_overlay_mismatch(policy: pixi_config::OverlayMismatch) -> rattler_fs::OverlayMismatch {
    match policy {
        pixi_config::OverlayMismatch::Error => rattler_fs::OverlayMismatch::Error,
        pixi_config::OverlayMismatch::Warn | pixi_config::OverlayMismatch::Ignore => {
            rattler_fs::OverlayMismatch::Adopt
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_interactive(
    lock_file: &rattler_lock::LockFile,
    environment_name: &str,
    platform: Platform,
    package_cache: &PackageCache,
    mount_point: &std::path::Path,
    overlay_dir: Option<PathBuf>,
    overlay_mismatch: rattler_fs::OverlayMismatch,
    env_hash: &str,
    transport: rattler_fs::Transport,
) -> miette::Result<()> {
    fs_err::create_dir_all(mount_point).into_diagnostic()?;

    let config = if let Some(overlay_dir) = overlay_dir {
        rattler_fs::MountConfig::new_writable(
            mount_point.to_path_buf(),
            Some(overlay_dir),
            transport,
            env_hash.to_string(),
        )
    } else {
        rattler_fs::MountConfig::new_read_only_if_supported(
            mount_point.to_path_buf(),
            transport,
            env_hash.to_string(),
        )
    }
    .with_overlay_mismatch(overlay_mismatch);

    let _handle = rattler_fs::build_and_mount(
        lock_file,
        environment_name,
        platform,
        package_cache,
        &config,
    )
    .await
    .map_err(|e| miette::miette!("failed to mount: {e}"))?;

    eprintln!(
        "Mounted at {}. Press Ctrl+C to unmount.",
        mount_point.display()
    );

    // Wait for Ctrl+C or SIGTERM
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .into_diagnostic()?;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = sigterm.recv() => {},
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await.into_diagnostic()?;

    // _handle drops here, triggering unmount
    Ok(())
}

#[allow(clippy::too_many_arguments, unused_variables)]
async fn execute_managed(
    lock_file: &rattler_lock::LockFile,
    environment_name: &str,
    platform: Platform,
    package_cache: &PackageCache,
    mount_point: &std::path::Path,
    overlay_dir: Option<PathBuf>,
    overlay_mismatch: rattler_fs::OverlayMismatch,
    env_hash: &str,
    lock_file_path: &std::path::Path,
    transport: rattler_fs::Transport,
    grace_period: u64,
    pidfile: Option<&std::path::Path>,
    ready_fd: Option<i32>,
    ready_event: Option<&str>,
) -> miette::Result<()> {
    fs_err::create_dir_all(mount_point).into_diagnostic()?;

    // Re-set CLOEXEC on the readiness pipe's write end. The parent handed it to
    // us with CLOEXEC cleared so we could inherit it across exec; re-set it now,
    // before mounting spawns helper processes (`sudo mount`/`fusermount3`), so
    // those helpers don't inherit it. An inherited, held write end would defer
    // the parent's readiness EOF and stall it for the full timeout.
    #[cfg(unix)]
    if let Some(fd) = ready_fd {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }

    // Read-only when there is no writable overlay. Captured before `overlay_dir`
    // is moved into the config below, so it can go into the sidecar record.
    let read_only = overlay_dir.is_none();

    let config = if let Some(overlay_dir) = overlay_dir {
        rattler_fs::MountConfig::new_writable(
            mount_point.to_path_buf(),
            Some(overlay_dir),
            transport,
            env_hash.to_string(),
        )
    } else {
        rattler_fs::MountConfig::new_read_only_if_supported(
            mount_point.to_path_buf(),
            transport,
            env_hash.to_string(),
        )
    }
    .with_overlay_mismatch(overlay_mismatch);

    let handle = rattler_fs::build_and_mount(
        lock_file,
        environment_name,
        platform,
        package_cache,
        &config,
    )
    .await
    .map_err(|e| miette::miette!("failed to mount: {e}"))?;

    // Write the sidecar record (pid + start time + identity). Written after the
    // mount is live and before signaling readiness, so a present, valid record
    // implies a mounted sidecar for this exact environment.
    if let Some(pidfile) = pidfile {
        pixi_core::environment::mount_sidecar::write_current_sidecar_state(
            pidfile,
            env_hash,
            lock_file_path,
            transport.resolve().name(),
            read_only,
            mount_point,
        )?;
    }

    // Signal readiness via pipe (Unix) or named event (Windows)
    #[cfg(unix)]
    if let Some(fd) = ready_fd {
        use std::os::unix::io::FromRawFd;
        let mut pipe = unsafe { std::fs::File::from_raw_fd(fd) };
        use std::io::Write;
        let _ = pipe.write_all(b"ready\n");
        // pipe is dropped/closed here
    }

    #[cfg(windows)]
    if let Some(event_name) = ready_event {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        let event_name_wide: Vec<u16> = OsStr::new(event_name)
            .encode_wide()
            .chain(Some(0))
            .collect();
        unsafe {
            let handle = windows_sys::Win32::System::Threading::OpenEventW(
                windows_sys::Win32::System::Threading::EVENT_MODIFY_STATE,
                0,
                event_name_wide.as_ptr(),
            );
            if !handle.is_null() {
                windows_sys::Win32::System::Threading::SetEvent(handle);
                windows_sys::Win32::Foundation::CloseHandle(handle);
            }
        }
    }

    // Poll for client activity using the lock file. When no clients hold a
    // shared lock for `grace_period` seconds, shut down.
    //
    // On grace-period expiry we keep the exclusive probe lock and carry it
    // (as `Some(probe_file)`) through teardown: an arriving client blocks on
    // it and, once we exit and the pidfile is gone, correctly respawns rather
    // than attaching to the mount we are tearing down. Signal-driven shutdown
    // holds nothing (`None`).
    #[cfg(unix)]
    let _teardown_lock: Option<std::fs::File> = {
        use std::os::unix::io::AsRawFd;

        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .into_diagnostic()?;

        let (lock_path, _) = pixi_core::environment::mount_sidecar::coordination_paths(mount_point);

        let probe_file = std::fs::OpenOptions::new()
            .read(true)
            .open(&lock_path)
            .into_diagnostic()?;
        let probe_fd = probe_file.as_raw_fd();

        let mut idle_seconds: u64 = 0;
        let mut probe_errors: u32 = 0;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        // Skip the immediate first tick to avoid racing with the parent
        // acquiring LOCK_SH after receiving the readiness signal.
        interval.tick().await;

        let hold_lock = loop {
            tokio::select! {
                _ = interval.tick() => {
                    let ret = unsafe { libc::flock(probe_fd, libc::LOCK_EX | libc::LOCK_NB) };
                    if ret == 0 {
                        // No clients hold shared locks.
                        idle_seconds += 1;
                        probe_errors = 0;
                        if idle_seconds >= grace_period {
                            tracing::info!(
                                "grace period expired ({grace_period}s), shutting down sidecar"
                            );
                            // Keep the exclusive lock through teardown.
                            break true;
                        }
                        // Not expired yet: release so clients can lock.
                        unsafe { libc::flock(probe_fd, libc::LOCK_UN); }
                    } else {
                        let err = std::io::Error::last_os_error();
                        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                            // A client holds a shared lock: reset the idle timer.
                            idle_seconds = 0;
                            probe_errors = 0;
                        } else {
                            // A genuine error (not contention). Don't treat it as
                            // "clients active" forever — that would make the
                            // sidecar immortal. Exit after a few consecutive ones.
                            probe_errors += 1;
                            tracing::warn!("sidecar lock probe failed: {err}");
                            if probe_errors >= 5 {
                                tracing::error!(
                                    "sidecar lock probe failing repeatedly; shutting down"
                                );
                                break false;
                            }
                        }
                    }
                }
                _ = sigterm.recv() => {
                    tracing::debug!("sidecar received SIGTERM, shutting down");
                    break false;
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::debug!("sidecar received Ctrl+C, shutting down");
                    break false;
                }
            }
        };

        if hold_lock { Some(probe_file) } else { None }
    };

    #[cfg(not(unix))]
    let _teardown_lock: Option<std::fs::File> = {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx, UnlockFileEx,
        };

        let (lock_path, _) = pixi_core::environment::mount_sidecar::coordination_paths(mount_point);

        let probe_file = std::fs::OpenOptions::new()
            .read(true)
            .open(&lock_path)
            .into_diagnostic()?;
        let probe_handle = probe_file.as_raw_handle();

        let mut idle_seconds: u64 = 0;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.tick().await;

        let hold_lock = loop {
            tokio::select! {
                _ = interval.tick() => {
                    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED =
                        unsafe { std::mem::zeroed() };
                    let got_exclusive = unsafe {
                        LockFileEx(
                            probe_handle,
                            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                            0,
                            1,
                            0,
                            &mut overlapped,
                        ) != 0
                    };
                    if got_exclusive {
                        idle_seconds += 1;
                        if idle_seconds >= grace_period {
                            tracing::info!(
                                "grace period expired ({grace_period}s), shutting down sidecar"
                            );
                            // Keep the exclusive lock through teardown (do not
                            // UnlockFileEx); released when the handle closes.
                            break true;
                        }
                        // Not expired yet: release so clients can lock.
                        unsafe { UnlockFileEx(probe_handle, 0, 1, 0, &mut overlapped); }
                    } else {
                        idle_seconds = 0;
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::debug!("sidecar received Ctrl+C, shutting down");
                    break false;
                }
            }
        };

        if hold_lock { Some(probe_file) } else { None }
    };

    // Tear down while still holding the probe lock (if we broke on grace
    // expiry). Unmount BEFORE removing the pidfile so a present, valid record
    // always implies a live mount. Explicit unmount surfaces errors that Drop
    // would silently swallow.
    if let Err(e) = handle.unmount().await {
        tracing::error!("sidecar unmount failed: {e}");
    }
    if let Some(pidfile) = pidfile {
        let _ = fs_err::remove_file(pidfile);
    }
    // The probe lock (if held) is released here, after teardown completed.
    drop(_teardown_lock);
    Ok(())
}
