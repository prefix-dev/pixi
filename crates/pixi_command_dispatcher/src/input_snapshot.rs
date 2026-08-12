//! The validation state of a cache entry's source input files, shared by the
//! build-backend metadata cache and the artifact cache.
//!
//! A snapshot is captured when an entry is stored and verified against the
//! filesystem when the entry is probed. Size and mtime are the fast path;
//! contents are hashed only when the mtime moved.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::file_fingerprint::{
    FileFingerprint, fingerprint_file, spawn_blocking_with_io_permit, system_time_serde,
};

/// Validation state recorded for one input file.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InputFileState {
    /// Content hash plus the size and mtime observed when it was computed.
    Fingerprint(FileFingerprint),
    /// Hashed after the build's cutoff (e.g. an mtime ahead of the clock),
    /// so the hash may not describe what the build read. Counts as changed
    /// until a later capture observes the identical fingerprint and promotes
    /// it: content that is stable across two builds is what the second build
    /// read.
    Unconfirmed(FileFingerprint),
    /// Only an mtime: the file could not be hashed when the entry was stored
    /// (unreadable or not a regular file). Any mtime change counts as a
    /// content change.
    MtimeOnly(#[serde(with = "system_time_serde")] SystemTime),
}

impl InputFileState {
    /// The mtime recorded for the file.
    pub(crate) fn modified(&self) -> SystemTime {
        match self {
            InputFileState::Fingerprint(fingerprint) | InputFileState::Unconfirmed(fingerprint) => {
                fingerprint.modified
            }
            InputFileState::MtimeOnly(modified) => *modified,
        }
    }

    /// The recorded fingerprint, if the file was hashed.
    fn fingerprint(&self) -> Option<&FileFingerprint> {
        match self {
            InputFileState::Fingerprint(fingerprint) | InputFileState::Unconfirmed(fingerprint) => {
                Some(fingerprint)
            }
            InputFileState::MtimeOnly(_) => None,
        }
    }

    /// Compares the recorded state against fresh stat data. `Unchanged` and
    /// `Changed` are final; `HashRequired` means the contents must be hashed
    /// to decide.
    fn compare_metadata(&self, metadata: &std::fs::Metadata) -> StateComparison {
        let Ok(modified) = metadata.modified() else {
            return StateComparison::Changed;
        };
        match self {
            InputFileState::Fingerprint(fingerprint) => {
                if metadata.len() != fingerprint.size {
                    StateComparison::Changed
                } else if modified == fingerprint.modified {
                    StateComparison::Unchanged
                } else if metadata.is_file() {
                    StateComparison::HashRequired
                } else {
                    // A special file cannot be hashed to prove its contents.
                    StateComparison::Changed
                }
            }
            // The hash cannot be trusted until a second capture confirms it.
            InputFileState::Unconfirmed(_) => StateComparison::Changed,
            InputFileState::MtimeOnly(expected) => {
                if modified == *expected {
                    StateComparison::Unchanged
                } else {
                    StateComparison::Changed
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum StateComparison {
    Unchanged,
    HashRequired,
    Changed,
}

/// The recorded validation state of every input file of a cache entry.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct InputSnapshot {
    files: BTreeMap<PathBuf, InputFileState>,
}

/// Result of [`InputSnapshot::verify`].
#[derive(Debug)]
pub(crate) enum SnapshotFreshness {
    /// Every file matches its recorded state.
    Fresh,
    /// Contents match but at least one mtime moved. Persisting the refreshed
    /// snapshot (best-effort) avoids rehashing on the next probe.
    Refreshed(InputSnapshot),
    /// The file provably changed, vanished, or could not be verified.
    Stale(StaleFile),
}

/// A file that failed verification, with the evidence for cache miss
/// reporting.
#[derive(Debug)]
pub(crate) struct StaleFile {
    pub(crate) path: PathBuf,
    pub(crate) reason: StaleFileReason,
}

/// Why verification rejected the file.
#[derive(Debug)]
pub(crate) enum StaleFileReason {
    /// The file can no longer be stat'ed or read.
    Removed,
    /// The recorded state no longer matches the file.
    Modified {
        recorded: SystemTime,
        observed: SystemTime,
    },
    /// The file was modified while the entry was being built; only a rebuild
    /// can confirm its contents.
    Unconfirmed,
}

impl StaleFile {
    fn removed(path: PathBuf) -> Self {
        Self {
            path,
            reason: StaleFileReason::Removed,
        }
    }

    fn modified(path: PathBuf, recorded: SystemTime, observed: SystemTime) -> Self {
        Self {
            path,
            reason: StaleFileReason::Modified { recorded, observed },
        }
    }
}

impl InputSnapshot {
    pub(crate) fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn get(&self, path: &std::path::Path) -> Option<&InputFileState> {
        self.files.get(path)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&PathBuf, &InputFileState)> {
        self.files.iter()
    }

    /// Inserts `state` for `path` unless a state is already recorded.
    pub(crate) fn insert_fallback(&mut self, path: PathBuf, state: InputFileState) {
        self.files.entry(path).or_insert(state);
    }

    /// Captures the current state of `paths`. Never fails: an unhashable
    /// file is recorded mtime-only and a file that cannot be stat'ed is
    /// dropped.
    ///
    /// A file modified after `cutoff` (an mtime ahead of the clock, or a file
    /// the build itself rewrote) may not match what the build read, so its
    /// hash is recorded as [`InputFileState::Unconfirmed`] and the entry
    /// rebuilds once more. When `previous` (the entry this one replaces)
    /// observed the same content, it was stable across both builds and the
    /// state is promoted to a trusted fingerprint. An edit in between changes
    /// the hash and breaks the promotion; an excursion that ends at the old
    /// content is the race the fast path already accepts everywhere. An
    /// mtime-only file past the cutoff keeps its mtime: it cannot be
    /// confirmed by a second observation, but it must stay tracked.
    pub(crate) async fn capture(
        paths: impl IntoIterator<Item = PathBuf>,
        cutoff: Option<SystemTime>,
        previous: Option<&InputSnapshot>,
        io_semaphore: Option<Arc<Semaphore>>,
    ) -> Self {
        let started = std::time::Instant::now();
        let outcomes = run_fingerprint_tasks(paths, io_semaphore, |path| {
            let state = match fingerprint_file(&path) {
                Ok(fingerprint) => Some(InputFileState::Fingerprint(fingerprint)),
                Err(err) => {
                    tracing::debug!(
                        path = %path.display(),
                        error = %err,
                        "could not hash source input file; keeping mtime-only validation"
                    );
                    fs_err::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .map(InputFileState::MtimeOnly)
                }
            };
            (path, state)
        })
        .await;

        let files: BTreeMap<_, _> = outcomes
            .into_iter()
            .filter_map(|(path, state)| {
                let state = state?;
                let state = match cutoff {
                    Some(cutoff) if state.modified() > cutoff => match state {
                        InputFileState::Fingerprint(fingerprint) => {
                            // Content equality across the two captures is
                            // what matters; the mtime may move (e.g. a build
                            // regenerating the file with identical bytes).
                            let confirmed = previous
                                .and_then(|previous| previous.files.get(&path))
                                .and_then(InputFileState::fingerprint)
                                .is_some_and(|prev| {
                                    prev.hash == fingerprint.hash && prev.size == fingerprint.size
                                });
                            if confirmed {
                                InputFileState::Fingerprint(fingerprint)
                            } else {
                                InputFileState::Unconfirmed(fingerprint)
                            }
                        }
                        // An mtime cannot be confirmed by a second
                        // observation, but dropping it would untrack the
                        // file; keep it validated by its mtime.
                        InputFileState::Unconfirmed(_) | InputFileState::MtimeOnly(_) => state,
                    },
                    _ => state,
                };
                Some((path, state))
            })
            .collect();
        log_hash_stats(
            files.values().map(|state| {
                state
                    .fingerprint()
                    .map_or(0, |fingerprint| fingerprint.size)
            }),
            started,
        );
        Self { files }
    }

    /// Verifies `files` against their recorded state.
    ///
    /// A file without a recorded state passes when its mtime is at or before
    /// `fallback_cutoff` (entries written before fingerprints existed);
    /// without a cutoff it counts as changed. Such files are never hashed:
    /// with nothing to compare against, a content hash proves nothing.
    pub(crate) async fn verify(
        &self,
        files: impl IntoIterator<Item = PathBuf>,
        fallback_cutoff: Option<SystemTime>,
        io_semaphore: Option<Arc<Semaphore>>,
    ) -> SnapshotFreshness {
        let mut to_hash = Vec::new();
        for path in files {
            let Ok(metadata) = fs_err::metadata(&path) else {
                return SnapshotFreshness::Stale(StaleFile::removed(path));
            };
            let observed = metadata.modified().ok();
            match self.files.get(&path) {
                Some(state) => match state.compare_metadata(&metadata) {
                    StateComparison::Unchanged => {}
                    StateComparison::HashRequired => to_hash.push(path),
                    StateComparison::Changed => {
                        if matches!(state, InputFileState::Unconfirmed(_)) {
                            return SnapshotFreshness::Stale(StaleFile {
                                path,
                                reason: StaleFileReason::Unconfirmed,
                            });
                        }
                        let recorded = state.modified();
                        return SnapshotFreshness::Stale(match observed {
                            Some(observed) => StaleFile::modified(path, recorded, observed),
                            None => StaleFile::removed(path),
                        });
                    }
                },
                None => {
                    let unchanged = fallback_cutoff
                        .is_some_and(|cutoff| observed.is_some_and(|modified| modified <= cutoff));
                    if !unchanged {
                        let Some(observed) = observed else {
                            return SnapshotFreshness::Stale(StaleFile::removed(path));
                        };
                        // With no recorded state the cutoff is the baseline.
                        let recorded = fallback_cutoff.unwrap_or(observed);
                        return SnapshotFreshness::Stale(StaleFile::modified(
                            path, recorded, observed,
                        ));
                    }
                }
            }
        }

        if to_hash.is_empty() {
            return SnapshotFreshness::Fresh;
        }

        let started = std::time::Instant::now();
        let hashed = run_fingerprint_tasks(to_hash, io_semaphore, |path| {
            let fingerprint = fingerprint_file(&path);
            (path, fingerprint)
        })
        .await;
        log_hash_stats(
            hashed
                .iter()
                .filter_map(|(_, fingerprint)| fingerprint.as_ref().ok())
                .map(|fingerprint| fingerprint.size),
            started,
        );

        let mut refreshed = self.clone();
        for (path, current) in hashed {
            let Ok(current) = current else {
                return SnapshotFreshness::Stale(StaleFile::removed(path));
            };
            match self.files.get(&path) {
                Some(InputFileState::Fingerprint(expected)) if expected.hash == current.hash => {
                    refreshed
                        .files
                        .insert(path, InputFileState::Fingerprint(current));
                }
                // Only files with a recorded fingerprint are queued for
                // hashing.
                Some(state) => {
                    return SnapshotFreshness::Stale(StaleFile::modified(
                        path,
                        state.modified(),
                        current.modified,
                    ));
                }
                None => return SnapshotFreshness::Stale(StaleFile::removed(path)),
            }
        }
        SnapshotFreshness::Refreshed(refreshed)
    }
}

impl FromIterator<(PathBuf, InputFileState)> for InputSnapshot {
    fn from_iter<T: IntoIterator<Item = (PathBuf, InputFileState)>>(iter: T) -> Self {
        Self {
            files: iter.into_iter().collect(),
        }
    }
}

fn log_hash_stats(sizes: impl Iterator<Item = u64>, started: std::time::Instant) {
    let (file_count, total_bytes) = sizes.fold((0u64, 0u64), |(n, b), size| (n + 1, b + size));
    tracing::debug!(
        file_count,
        total_bytes,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "fingerprinted source-build input files"
    );
}

/// Runs `task` for each path on the blocking pool, each run holding one
/// permit from the dispatcher's shared I/O budget. A permit covers one file
/// read, so other users of the budget can make progress between files.
async fn run_fingerprint_tasks<T: Send + 'static>(
    paths: impl IntoIterator<Item = PathBuf>,
    io_semaphore: Option<Arc<Semaphore>>,
    task: impl Fn(PathBuf) -> T + Clone + Send + 'static,
) -> Vec<T> {
    use futures::StreamExt;

    let paths = paths.into_iter().collect::<std::collections::BTreeSet<_>>();
    let worker_count = paths
        .len()
        .min(
            std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
        )
        .max(1);
    futures::stream::iter(paths)
        .map(|path| {
            let io_semaphore = io_semaphore.clone();
            let task = task.clone();
            async move {
                spawn_blocking_with_io_permit(io_semaphore, move || task(path))
                    .await
                    .expect("file fingerprint task panicked")
            }
        })
        .buffer_unordered(worker_count)
        .collect()
        .await
}

#[cfg(test)]
mod tests {
    use std::{fs::OpenOptions, time::Duration};

    use tempfile::TempDir;

    use super::*;

    fn set_modified(path: &std::path::Path, modified: SystemTime) {
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    fn mtime(path: &std::path::Path) -> SystemTime {
        fs_err::metadata(path).unwrap().modified().unwrap()
    }

    #[tokio::test]
    async fn capture_records_fingerprints_for_regular_files() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        let snapshot = InputSnapshot::capture([file.clone()], None, None, None).await;
        assert!(matches!(
            snapshot.get(&file),
            Some(InputFileState::Fingerprint(_))
        ));
    }

    #[tokio::test]
    async fn capture_falls_back_to_mtime_for_unhashable_files() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("assets");
        fs_err::create_dir_all(&dir).unwrap();

        let snapshot = InputSnapshot::capture([dir.clone()], None, None, None).await;
        assert!(matches!(
            snapshot.get(&dir),
            Some(InputFileState::MtimeOnly(_))
        ));
    }

    #[tokio::test]
    async fn capture_drops_missing_files() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("gone.py");

        let snapshot = InputSnapshot::capture([missing], None, None, None).await;
        assert!(snapshot.is_empty());
    }

    #[tokio::test]
    async fn capture_records_files_modified_after_the_cutoff_as_unconfirmed() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        let cutoff = mtime(&file) - Duration::from_secs(10);
        let snapshot = InputSnapshot::capture([file.clone()], Some(cutoff), None, None).await;
        assert!(matches!(
            snapshot.get(&file),
            Some(InputFileState::Unconfirmed(_))
        ));

        // An unconfirmed state cannot vouch for the cached outputs.
        assert!(matches!(
            snapshot.verify([file], Some(cutoff), None).await,
            SnapshotFreshness::Stale(_)
        ));
    }

    /// The two-observation promotion: content that is stable across two
    /// captures is what the second build read, even when the mtime stays
    /// past the cutoff or moves between the builds.
    #[tokio::test]
    async fn capture_promotes_an_unconfirmed_fingerprint_seen_twice() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();
        let cutoff = mtime(&file) - Duration::from_secs(10);

        let first = InputSnapshot::capture([file.clone()], Some(cutoff), None, None).await;
        set_modified(&file, mtime(&file) + Duration::from_secs(1));
        let second = InputSnapshot::capture([file.clone()], Some(cutoff), Some(&first), None).await;

        assert!(matches!(
            second.get(&file),
            Some(InputFileState::Fingerprint(_))
        ));
        assert!(matches!(
            second.verify([file], Some(cutoff), None).await,
            SnapshotFreshness::Fresh
        ));
    }

    #[tokio::test]
    async fn capture_does_not_promote_changed_content() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"old").unwrap();
        let cutoff = mtime(&file) - Duration::from_secs(10);

        let first = InputSnapshot::capture([file.clone()], Some(cutoff), None, None).await;
        fs_err::write(&file, b"new").unwrap();
        set_modified(&file, mtime(&file) + Duration::from_secs(1));
        let second = InputSnapshot::capture([file.clone()], Some(cutoff), Some(&first), None).await;

        assert!(matches!(
            second.get(&file),
            Some(InputFileState::Unconfirmed(_))
        ));
    }

    #[tokio::test]
    async fn verify_is_fresh_for_unchanged_files() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        let snapshot = InputSnapshot::capture([file.clone()], None, None, None).await;
        assert!(matches!(
            snapshot.verify([file], None, None).await,
            SnapshotFreshness::Fresh
        ));
    }

    #[tokio::test]
    async fn verify_refreshes_a_touched_but_unchanged_file() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        let snapshot = InputSnapshot::capture([file.clone()], None, None, None).await;
        let touched = mtime(&file) + Duration::from_secs(1);
        set_modified(&file, touched);

        match snapshot.verify([file.clone()], None, None).await {
            SnapshotFreshness::Refreshed(refreshed) => {
                assert_eq!(refreshed.get(&file).unwrap().modified(), touched);
            }
            other => panic!("expected a refresh, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn verify_is_stale_for_a_same_size_edit() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"old").unwrap();

        let snapshot = InputSnapshot::capture([file.clone()], None, None, None).await;
        let touched = mtime(&file) + Duration::from_secs(1);
        fs_err::write(&file, b"new").unwrap();
        set_modified(&file, touched);

        assert!(matches!(
            snapshot.verify([file], None, None).await,
            SnapshotFreshness::Stale(_)
        ));
    }

    #[tokio::test]
    async fn verify_is_stale_for_a_deleted_file() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        let snapshot = InputSnapshot::capture([file.clone()], None, None, None).await;
        fs_err::remove_file(&file).unwrap();

        assert!(matches!(
            snapshot.verify([file], None, None).await,
            SnapshotFreshness::Stale(_)
        ));
    }

    #[tokio::test]
    async fn verify_is_stale_when_an_mtime_only_file_is_touched() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("assets");
        fs_err::create_dir_all(&dir).unwrap();

        let snapshot = InputSnapshot::capture([dir.clone()], None, None, None).await;
        assert!(matches!(
            snapshot.verify([dir.clone()], None, None).await,
            SnapshotFreshness::Fresh
        ));

        // A recorded mtime that no longer matches must invalidate; an
        // mtime-only file can never be re-verified through a hash.
        let touched = InputFileState::MtimeOnly(mtime(&dir) + Duration::from_secs(1));
        let snapshot = InputSnapshot::from_iter([(dir.clone(), touched)]);
        assert!(matches!(
            snapshot.verify([dir], None, None).await,
            SnapshotFreshness::Stale(_)
        ));
    }

    /// Files without a recorded state (entries written before fingerprints
    /// existed) pass the cutoff check but never gain a hash: with nothing to
    /// compare against, a hash of the current contents proves nothing.
    #[tokio::test]
    async fn verify_uses_the_cutoff_for_unrecorded_files_and_never_hashes_them() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();
        let snapshot = InputSnapshot::default();

        let cutoff = mtime(&file) + Duration::from_secs(10);
        assert!(matches!(
            snapshot.verify([file.clone()], Some(cutoff), None).await,
            SnapshotFreshness::Fresh
        ));

        assert!(matches!(
            snapshot.verify([file.clone()], None, None).await,
            SnapshotFreshness::Stale(_)
        ));

        set_modified(&file, cutoff + Duration::from_secs(10));
        assert!(matches!(
            snapshot.verify([file], Some(cutoff), None).await,
            SnapshotFreshness::Stale(_)
        ));
    }

    /// Promotion only asks whether the previous capture saw the same content;
    /// it does not care whether that previous state was itself trusted. A file
    /// whose mtime stays past the cutoff forever must therefore keep its
    /// trusted fingerprint instead of falling back to unconfirmed.
    #[tokio::test]
    async fn capture_promotes_when_the_previous_state_is_a_trusted_fingerprint() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();
        let cutoff = mtime(&file) - Duration::from_secs(10);

        let first = InputSnapshot::capture([file.clone()], None, None, None).await;
        assert!(matches!(
            first.get(&file),
            Some(InputFileState::Fingerprint(_))
        ));

        let second = InputSnapshot::capture([file.clone()], Some(cutoff), Some(&first), None).await;
        assert!(
            matches!(second.get(&file), Some(InputFileState::Fingerprint(_))),
            "a trusted previous observation of the same content must promote",
        );

        // And the trust survives further captures that keep seeing it.
        let third = InputSnapshot::capture([file.clone()], Some(cutoff), Some(&second), None).await;
        assert!(matches!(
            third.get(&file),
            Some(InputFileState::Fingerprint(_))
        ));
    }

    /// A path that previously held something unhashable (a directory) carries
    /// no fingerprint, so when a regular file takes its place past the cutoff
    /// there is nothing to confirm it against.
    #[tokio::test]
    async fn capture_does_not_promote_a_path_that_was_previously_mtime_only() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("assets");
        fs_err::create_dir_all(&path).unwrap();

        let first = InputSnapshot::capture([path.clone()], None, None, None).await;
        assert!(matches!(
            first.get(&path),
            Some(InputFileState::MtimeOnly(_))
        ));

        fs_err::remove_dir(&path).unwrap();
        fs_err::write(&path, b"body").unwrap();
        let cutoff = mtime(&path) - Duration::from_secs(10);

        let second = InputSnapshot::capture([path.clone()], Some(cutoff), Some(&first), None).await;
        assert!(
            matches!(second.get(&path), Some(InputFileState::Unconfirmed(_))),
            "an mtime-only predecessor cannot vouch for any content",
        );
    }

    /// Every empty file shares one hash, so content equality alone would let
    /// any empty input confirm any other. Promotion is keyed by path, which is
    /// what keeps them apart.
    #[tokio::test]
    async fn capture_promotion_is_keyed_by_path_not_by_content() {
        let temp = TempDir::new().unwrap();
        let seen = temp.path().join("seen.py");
        let fresh = temp.path().join("fresh.py");
        fs_err::write(&seen, b"").unwrap();
        fs_err::write(&fresh, b"").unwrap();
        let cutoff = mtime(&seen).min(mtime(&fresh)) - Duration::from_secs(10);

        let previous = InputSnapshot::capture([seen.clone()], None, None, None).await;
        let captured = InputSnapshot::capture(
            [seen.clone(), fresh.clone()],
            Some(cutoff),
            Some(&previous),
            None,
        )
        .await;

        assert!(
            matches!(captured.get(&seen), Some(InputFileState::Fingerprint(_))),
            "the path the previous capture recorded must be promoted",
        );
        assert!(
            matches!(captured.get(&fresh), Some(InputFileState::Unconfirmed(_))),
            "an identical-but-unseen path must not borrow another path's confirmation",
        );
    }

    /// The cutoff is exclusive: a file whose mtime lands exactly on it was
    /// not modified after the build started.
    #[tokio::test]
    async fn capture_treats_an_mtime_equal_to_the_cutoff_as_trusted() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        let cutoff = mtime(&file);
        let snapshot = InputSnapshot::capture([file.clone()], Some(cutoff), None, None).await;
        assert!(matches!(
            snapshot.get(&file),
            Some(InputFileState::Fingerprint(_))
        ));

        set_modified(&file, cutoff + Duration::from_secs(1));
        let snapshot = InputSnapshot::capture([file.clone()], Some(cutoff), None, None).await;
        assert!(matches!(
            snapshot.get(&file),
            Some(InputFileState::Unconfirmed(_))
        ));
    }

    /// An unhashable input (a directory) whose mtime is past the cutoff must
    /// stay in the snapshot: dropping it would silently remove it from
    /// validation. The recorded mtime is no less trustworthy than one
    /// recorded before the cutoff.
    #[tokio::test]
    async fn capture_must_not_silently_untrack_an_mtime_only_file_past_the_cutoff() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("assets");
        fs_err::create_dir_all(&dir).unwrap();
        let cutoff = mtime(&dir) - Duration::from_secs(10);

        let snapshot = InputSnapshot::capture([dir.clone()], Some(cutoff), None, None).await;
        assert!(
            matches!(snapshot.get(&dir), Some(InputFileState::MtimeOnly(_))),
            "dropping the input leaves it unvalidated forever, which is weaker \
             than any recorded state",
        );
    }
}
