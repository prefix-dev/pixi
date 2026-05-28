//! Cross-process exclusive lock on an environment prefix. The lock
//! file doubles as a small state marker, holding one of:
//!
//! - empty: nothing installed yet;
//! - the in-progress marker: an install started but did not finish
//!   (the writer crashed), so the prefix may be partially written;
//! - a fixed-width fingerprint: an install completed successfully.
//!
//! A peer that just finished installing the same spec is observed via
//! `matches` under the lock, so the second arrival skips redundant
//! work. A leftover in-progress marker tells the next arrival the
//! prefix is dirty and must be fully reinstalled.
//!
//! Writes are in place at offset 0. The fixed [`FINGERPRINT_WIDTH`] is
//! load-bearing: a single small `write` is observed atomically by
//! unlocked readers, so changing the width could expose torn reads.

use std::{io, path::Path, time::Duration};

use async_fd_lock::{LockWrite, RwLockWriteGuard};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    time::Instant,
};

use crate::EnvironmentFingerprint;

/// Fixed on-disk marker width, in bytes. Unlocked readers depend on
/// this width staying constant for torn-read safety.
pub(crate) const FINGERPRINT_WIDTH: usize = 16;

/// Filename of the on-disk marker, under `<prefix>/conda-meta/`.
const MARKER_FILENAME: &str = ".pixi-environment-fingerprint";

/// Written while an install is in progress. Distinguishable from a
/// completed fingerprint because it is not all hex digits.
const IN_PROGRESS_MARKER: [u8; FINGERPRINT_WIDTH] = *b"pixi:installing!";

pub(crate) fn marker_path(prefix_dir: &Path) -> std::path::PathBuf {
    prefix_dir.join("conda-meta").join(MARKER_FILENAME)
}

/// State the lock file held when the lock was acquired, i.e. what the
/// previous holder left behind.
enum MarkerState {
    /// Empty file: no install has run here.
    Fresh,
    /// In-progress marker (or anything not a valid fingerprint): a
    /// previous install was interrupted and the prefix may be dirty.
    Interrupted,
    /// A completed install recorded this fingerprint.
    Installed(EnvironmentFingerprint),
}

pub struct EnvironmentLock {
    file: RwLockWriteGuard<File>,
    /// What the previous holder left, captured at acquire time. Stays
    /// fixed across `begin` so [`Self::was_interrupted`] always
    /// reflects the inherited state, not our own marker.
    inherited: MarkerState,
}

impl EnvironmentLock {
    /// Acquire the exclusive lock for the prefix at `prefix_dir`,
    /// creating the marker file and `conda-meta/` if absent. Blocks
    /// until peer writers release.
    pub async fn acquire(prefix_dir: &Path) -> io::Result<Self> {
        let file = open_lock_file(&marker_path(prefix_dir)).await?;
        Self::from_locked(file.lock_write().await?).await
    }

    /// Like [`Self::acquire`] but invokes `on_waiting(elapsed)` every
    /// `interval` while still blocked, for progress feedback.
    pub async fn acquire_with_progress<F: FnMut(Duration)>(
        prefix_dir: &Path,
        interval: Duration,
        mut on_waiting: F,
    ) -> io::Result<Self> {
        let file = open_lock_file(&marker_path(prefix_dir)).await?;
        let start = Instant::now();
        let lock_fut = file.lock_write();
        tokio::pin!(lock_fut);
        loop {
            tokio::select! {
                biased;
                result = &mut lock_fut => {
                    return Self::from_locked(result.map_err(io::Error::from)?).await;
                }
                _ = tokio::time::sleep(interval) => {
                    on_waiting(start.elapsed());
                }
            }
        }
    }

    async fn from_locked(mut file: RwLockWriteGuard<File>) -> io::Result<Self> {
        file.rewind().await?;
        let mut buf = [0u8; FINGERPRINT_WIDTH];
        let inherited = match file.read_exact(&mut buf).await {
            Ok(_) if buf.iter().all(u8::is_ascii_hexdigit) => MarkerState::Installed(
                EnvironmentFingerprint::from_string(String::from_utf8_lossy(&buf).into_owned()),
            ),
            Ok(_) => MarkerState::Interrupted,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => MarkerState::Fresh,
            Err(e) => return Err(e),
        };
        Ok(Self { file, inherited })
    }

    /// True iff a completed install recorded exactly `expected`.
    pub fn matches(&self, expected: &EnvironmentFingerprint) -> bool {
        matches!(&self.inherited, MarkerState::Installed(fp) if fp == expected)
    }

    /// The fingerprint of the last completed install, if any. `None`
    /// for a fresh prefix or one left dirty by an interrupted install.
    pub fn current(&self) -> Option<&EnvironmentFingerprint> {
        match &self.inherited {
            MarkerState::Installed(fp) => Some(fp),
            _ => None,
        }
    }

    /// True iff a previous install was interrupted, leaving the prefix
    /// possibly partially written. Callers should reinstall fully.
    pub fn was_interrupted(&self) -> bool {
        matches!(self.inherited, MarkerState::Interrupted)
    }

    /// Mark the prefix as being installed into. Call before mutating
    /// the prefix; pair with [`Self::finish`] on success. If the
    /// process crashes in between, the next [`Self::acquire`] sees
    /// [`Self::was_interrupted`].
    pub async fn begin(&mut self) -> io::Result<()> {
        self.write_marker(&IN_PROGRESS_MARKER).await
    }

    /// Record `fingerprint` as the new on-disk state and release the
    /// lock. In-place write at offset 0; safe for unlocked readers
    /// (see the module doc).
    pub async fn finish(mut self, fingerprint: &EnvironmentFingerprint) -> io::Result<()> {
        self.write_marker(&fingerprint.as_bytes()).await
    }

    async fn write_marker(&mut self, bytes: &[u8; FINGERPRINT_WIDTH]) -> io::Result<()> {
        self.file.rewind().await?;
        self.file.write_all(bytes).await?;
        self.file.flush().await?;
        Ok(())
    }
}

/// Open (creating if absent) the lock file at `path`.
async fn open_lock_file(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs_err::tokio::create_dir_all(parent).await?;
    }
    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fp(s: &str) -> EnvironmentFingerprint {
        EnvironmentFingerprint::from_string(s.to_string())
    }

    #[tokio::test]
    async fn fresh_file_does_not_match() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("env");
        let lock = EnvironmentLock::acquire(&path).await.unwrap();
        assert!(!lock.matches(&fp("0123456789abcdef")));
        assert!(lock.current().is_none());
        assert!(!lock.was_interrupted());
    }

    #[tokio::test]
    async fn finish_then_match() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("env");
        let a = fp("0123456789abcdef");
        let b = fp("fedcba9876543210");

        let mut lock = EnvironmentLock::acquire(&path).await.unwrap();
        lock.begin().await.unwrap();
        lock.finish(&a).await.unwrap();

        let lock = EnvironmentLock::acquire(&path).await.unwrap();
        assert!(lock.matches(&a));
        assert!(!lock.matches(&b));
        assert_eq!(lock.current(), Some(&a));
        assert!(!lock.was_interrupted());
    }

    #[tokio::test]
    async fn begin_without_finish_is_interrupted() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("env");

        // Simulate a crash: begin, then drop the lock without finish.
        let mut lock = EnvironmentLock::acquire(&path).await.unwrap();
        lock.begin().await.unwrap();
        drop(lock);

        // The next acquire sees the prefix as dirty.
        let lock = EnvironmentLock::acquire(&path).await.unwrap();
        assert!(lock.was_interrupted());
        assert!(lock.current().is_none());
        // And a lock-free read does not mistake the marker for a hit.
        assert!(EnvironmentFingerprint::read(&path).is_none());
    }

    #[tokio::test]
    async fn lockfree_read_via_fingerprint() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("env");
        assert!(EnvironmentFingerprint::read(&path).is_none());

        let a = fp("0123456789abcdef");
        let mut lock = EnvironmentLock::acquire(&path).await.unwrap();
        lock.begin().await.unwrap();
        lock.finish(&a).await.unwrap();

        assert_eq!(EnvironmentFingerprint::read(&path), Some(a));
    }

    #[tokio::test]
    async fn progress_callback_fires_while_blocked() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("env");

        let held = EnvironmentLock::acquire(&path).await.unwrap();

        let path2 = path.clone();
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let waiter = tokio::spawn(async move {
            EnvironmentLock::acquire_with_progress(&path2, Duration::from_millis(20), move |_| {
                counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
            .await
            .unwrap()
            .finish(&fp("0123456789abcdef"))
            .await
            .unwrap();
        });

        tokio::time::sleep(Duration::from_millis(120)).await;
        let fired = counter.load(std::sync::atomic::Ordering::SeqCst);
        drop(held);
        waiter.await.unwrap();
        assert!(
            fired >= 2,
            "expected progress callback to fire at least twice while blocked, got {fired}",
        );
    }
}
