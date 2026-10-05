//! The validation state of a cache entry's source input files, shared by the
//! build-backend metadata cache and the artifact cache.
//!
//! A snapshot is captured when an entry is stored and verified against the
//! filesystem when the entry is probed. Size and mtime are the fast path;
//! contents are hashed only when the mtime moved.

use std::{collections::BTreeMap, path::Path, sync::Arc, time::SystemTime};

use pixi_path::AbsPathBuf;
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
pub(crate) struct InputSnapshot {
    files: BTreeMap<AbsPathBuf, InputFileState>,

    /// When the states were observed. An unconfirmed fingerprint in this
    /// snapshot may only confirm a later capture whose build started after
    /// this; otherwise the two observations do not bracket that build's
    /// reads. Absent for snapshots that never ran a capture.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "system_time_serde::optional"
    )]
    captured_at: Option<SystemTime>,
}

/// Result of [`InputSnapshot::verify`].
#[derive(Debug)]
pub(crate) enum SnapshotFreshness {
    /// Every file matches its recorded state.
    Fresh,
    /// These files' contents match but their mtime moved. Recording them
    /// (best-effort) via [`InputSnapshot::apply_refresh`] avoids rehashing
    /// on the next probe.
    Refreshed(Vec<(AbsPathBuf, FileFingerprint)>),
    /// The file provably changed, vanished, or could not be verified.
    Stale(StaleFile),
}

/// A file that failed verification, with the evidence for cache miss
/// reporting.
#[derive(Debug)]
pub(crate) struct StaleFile {
    pub(crate) path: AbsPathBuf,
    pub(crate) reason: StaleFileReason,
}

/// Why verification rejected the file.
#[derive(Debug)]
pub(crate) enum StaleFileReason {
    /// The file no longer exists.
    Removed,
    /// The file exists but its contents or metadata could not be read.
    Unreadable,
    /// The recorded state no longer matches the file.
    Modified {
        recorded: SystemTime,
        observed: SystemTime,
    },
    /// The entry records no state for the file, so nothing can verify it.
    Untracked,
    /// The file was modified while the entry was being built; only a rebuild
    /// can confirm its contents.
    Unconfirmed,
}

impl StaleFile {
    fn removed(path: AbsPathBuf) -> Self {
        Self {
            path,
            reason: StaleFileReason::Removed,
        }
    }

    fn unreadable(path: AbsPathBuf) -> Self {
        Self {
            path,
            reason: StaleFileReason::Unreadable,
        }
    }

    fn untracked(path: AbsPathBuf) -> Self {
        Self {
            path,
            reason: StaleFileReason::Untracked,
        }
    }

    fn modified(path: AbsPathBuf, recorded: SystemTime, observed: SystemTime) -> Self {
        Self {
            path,
            reason: StaleFileReason::Modified { recorded, observed },
        }
    }

    /// A missing file was removed; any other I/O failure means the file is
    /// still there but cannot be verified.
    fn from_io(path: AbsPathBuf, kind: std::io::ErrorKind) -> Self {
        if kind == std::io::ErrorKind::NotFound {
            Self::removed(path)
        } else {
            Self::unreadable(path)
        }
    }
}

impl InputSnapshot {
    pub(crate) fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn get(&self, path: &Path) -> Option<&InputFileState> {
        self.files
            .iter()
            .find(|(recorded, _)| recorded.as_std_path() == path)
            .map(|(_, state)| state)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&AbsPathBuf, &InputFileState)> {
        self.files.iter()
    }

    /// Inserts `state` for `path` unless a state is already recorded.
    pub(crate) fn insert_fallback(&mut self, path: AbsPathBuf, state: InputFileState) {
        self.files.entry(path).or_insert(state);
    }

    /// Records freshly verified fingerprints from a
    /// [`SnapshotFreshness::Refreshed`] result.
    pub(crate) fn apply_refresh(
        &mut self,
        refreshed: impl IntoIterator<Item = (AbsPathBuf, FileFingerprint)>,
    ) {
        for (path, fingerprint) in refreshed {
            self.files
                .insert(path, InputFileState::Fingerprint(fingerprint));
        }
    }

    /// Captures the current state of `paths`. Never fails: an unhashable
    /// file is recorded mtime-only and a file that cannot be stat'ed is
    /// dropped.
    ///
    /// A file modified after `cutoff` (an mtime ahead of the clock, or a file
    /// the build itself rewrote) may not match what the build read, so its
    /// hash is recorded as [`InputFileState::Unconfirmed`] and the entry
    /// rebuilds once more. When `previous` (the entry this one replaces)
    /// observed the same content *before this build started*, the content
    /// was stable while the build read it and the state is promoted to a
    /// trusted fingerprint. Both halves matter: without the time bound, a
    /// concurrent build that read older content could have its artifact
    /// vouched for by another process's observation. An edit in between
    /// changes the hash and breaks the promotion; an excursion that ends at
    /// the old content is the race the fast path already accepts everywhere.
    /// An mtime-only file past the cutoff keeps its mtime: it cannot be
    /// confirmed by a second observation, but it must stay tracked.
    pub(crate) async fn capture(
        paths: impl IntoIterator<Item = AbsPathBuf>,
        cutoff: Option<SystemTime>,
        previous: Option<&InputSnapshot>,
        io_semaphore: Option<Arc<Semaphore>>,
    ) -> Self {
        let started = std::time::Instant::now();
        let outcomes =
            run_fingerprint_tasks(paths, io_semaphore, |path| match fingerprint_file(path) {
                Ok(fingerprint) => Some(InputFileState::Fingerprint(fingerprint)),
                Err(err) => {
                    tracing::debug!(
                        path = %path.display(),
                        error = %err,
                        "could not hash source input file; keeping mtime-only validation"
                    );
                    fs_err::metadata(path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .map(InputFileState::MtimeOnly)
                }
            })
            .await;

        let files: BTreeMap<_, _> = outcomes
            .into_iter()
            .filter_map(|(path, state)| {
                let state = match (cutoff, state?) {
                    (Some(cutoff), InputFileState::Fingerprint(fingerprint))
                        if fingerprint.modified > cutoff
                            && !confirmed_by_previous(previous, cutoff, &path, &fingerprint) =>
                    {
                        InputFileState::Unconfirmed(fingerprint)
                    }
                    // An mtime past the cutoff cannot be confirmed by a
                    // second observation, but the file must stay tracked.
                    (_, state) => state,
                };
                Some((path, state))
            })
            .collect();
        log_hash_stats(
            files.values().filter_map(InputFileState::fingerprint),
            started,
        );
        Self {
            files,
            // Taken after all hashing: every observation predates it.
            captured_at: Some(SystemTime::now()),
        }
    }

    /// Verifies `files` against their recorded state.
    ///
    /// A file without a recorded state passes when its mtime is at or before
    /// `fallback_cutoff` (entries written before fingerprints existed);
    /// without a cutoff it counts as changed. Such files are never hashed:
    /// with nothing to compare against, a content hash proves nothing.
    pub(crate) async fn verify(
        &self,
        files: impl IntoIterator<Item = AbsPathBuf>,
        fallback_cutoff: Option<SystemTime>,
        io_semaphore: Option<Arc<Semaphore>>,
    ) -> SnapshotFreshness {
        // One blocking task for the whole stat pass: it keeps the syscalls
        // off the async worker and yields immediately, so work joined with
        // this future (the glob walk) genuinely runs concurrently.
        let states: Vec<(AbsPathBuf, Option<InputFileState>)> = files
            .into_iter()
            .map(|path| {
                let state = self.files.get(&path).copied();
                (path, state)
            })
            .collect();
        let to_hash =
            spawn_blocking_with_io_permit(None, move || classify_files(states, fallback_cutoff))
                .await
                .expect("file stat task panicked");
        let to_hash = match to_hash {
            Ok(to_hash) => to_hash,
            Err(stale) => return SnapshotFreshness::Stale(stale),
        };

        if to_hash.is_empty() {
            return SnapshotFreshness::Fresh;
        }

        let started = std::time::Instant::now();
        let hashed = run_fingerprint_tasks(
            to_hash.keys().cloned().collect::<Vec<_>>(),
            io_semaphore,
            fingerprint_file,
        )
        .await;
        log_hash_stats(
            hashed
                .iter()
                .filter_map(|(_, fingerprint)| fingerprint.as_ref().ok()),
            started,
        );

        let mut refreshed = Vec::with_capacity(hashed.len());
        for (path, current) in hashed {
            let current = match current {
                Ok(current) => current,
                Err(err) => {
                    return SnapshotFreshness::Stale(StaleFile::from_io(path, err.source.kind()));
                }
            };
            let expected = to_hash[&path];
            if expected.hash != current.hash {
                return SnapshotFreshness::Stale(StaleFile::modified(
                    path,
                    expected.modified,
                    current.modified,
                ));
            }
            tracing::debug!(
                path = %path,
                recorded_mtime = %chrono::DateTime::<chrono::Utc>::from(expected.modified),
                observed_mtime = %chrono::DateTime::<chrono::Utc>::from(current.modified),
                "input file mtime moved but its contents are unchanged; keeping the cache hit"
            );
            refreshed.push((path, current));
        }
        SnapshotFreshness::Refreshed(refreshed)
    }
}

/// Whether `previous` proves that `fingerprint`'s content was stable while
/// the build ran: it must have observed the same content, and must have
/// observed it before the build started (the cutoff).
fn confirmed_by_previous(
    previous: Option<&InputSnapshot>,
    cutoff: SystemTime,
    path: &AbsPathBuf,
    fingerprint: &FileFingerprint,
) -> bool {
    previous
        .filter(|previous| {
            previous
                .captured_at
                .is_some_and(|observed| observed <= cutoff)
        })
        .and_then(|previous| previous.files.get(path))
        .and_then(InputFileState::fingerprint)
        .is_some_and(|prev| prev.hash == fingerprint.hash && prev.size == fingerprint.size)
}

/// The stat pass of [`InputSnapshot::verify`]: compares every file against
/// its recorded state and returns the fingerprints that need a content hash
/// to decide.
fn classify_files(
    states: Vec<(AbsPathBuf, Option<InputFileState>)>,
    fallback_cutoff: Option<SystemTime>,
) -> Result<BTreeMap<AbsPathBuf, FileFingerprint>, StaleFile> {
    let mut to_hash = BTreeMap::new();
    for (path, state) in states {
        let metadata = match fs_err::metadata(path.as_std_path()) {
            Ok(metadata) => metadata,
            Err(err) => return Err(StaleFile::from_io(path, err.kind())),
        };
        let observed = metadata.modified().ok();
        match state {
            Some(state) => match state.compare_metadata(&metadata) {
                StateComparison::Unchanged => {}
                StateComparison::HashRequired => {
                    let fingerprint = *state
                        .fingerprint()
                        .expect("only fingerprints can require a hash");
                    to_hash.insert(path, fingerprint);
                }
                StateComparison::Changed => {
                    if matches!(state, InputFileState::Unconfirmed(_)) {
                        return Err(StaleFile {
                            path,
                            reason: StaleFileReason::Unconfirmed,
                        });
                    }
                    let recorded = state.modified();
                    return Err(match observed {
                        Some(observed) => StaleFile::modified(path, recorded, observed),
                        None => StaleFile::unreadable(path),
                    });
                }
            },
            None => {
                let unchanged = fallback_cutoff
                    .is_some_and(|cutoff| observed.is_some_and(|modified| modified <= cutoff));
                if !unchanged {
                    return Err(match (fallback_cutoff, observed) {
                        // With no recorded state the cutoff is the baseline.
                        (Some(cutoff), Some(observed)) => {
                            StaleFile::modified(path, cutoff, observed)
                        }
                        (None, _) => StaleFile::untracked(path),
                        (_, None) => StaleFile::unreadable(path),
                    });
                }
            }
        }
    }
    Ok(to_hash)
}

#[cfg(test)]
impl FromIterator<(AbsPathBuf, InputFileState)> for InputSnapshot {
    fn from_iter<T: IntoIterator<Item = (AbsPathBuf, InputFileState)>>(iter: T) -> Self {
        Self {
            files: iter.into_iter().collect(),
            captured_at: None,
        }
    }
}

fn log_hash_stats<'a>(
    fingerprints: impl Iterator<Item = &'a FileFingerprint>,
    started: std::time::Instant,
) {
    let (file_count, total_bytes) = fingerprints
        .fold((0u64, 0u64), |(count, bytes), fingerprint| {
            (count + 1, bytes + fingerprint.size)
        });
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
    paths: impl IntoIterator<Item = AbsPathBuf>,
    io_semaphore: Option<Arc<Semaphore>>,
    task: impl Fn(&Path) -> T + Clone + Send + 'static,
) -> Vec<(AbsPathBuf, T)> {
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
                spawn_blocking_with_io_permit(io_semaphore, move || {
                    let outcome = task(path.as_std_path());
                    (path, outcome)
                })
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
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;
    use crate::file_fingerprint::test_helpers::{mtime, set_modified};

    fn abs(path: &Path) -> AbsPathBuf {
        AbsPathBuf::new(path.to_path_buf()).unwrap()
    }

    /// Dates the file ahead of the clock, so it lands past any cutoff taken
    /// "now" (a skewed mount, a restored snapshot).
    fn set_future_mtime(path: &Path) {
        set_modified(path, SystemTime::now() + Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn capture_records_fingerprints_for_regular_files() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        let snapshot = InputSnapshot::capture([abs(&file)], None, None, None).await;
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

        let snapshot = InputSnapshot::capture([abs(&dir)], None, None, None).await;
        assert!(matches!(
            snapshot.get(&dir),
            Some(InputFileState::MtimeOnly(_))
        ));
    }

    #[tokio::test]
    async fn capture_drops_missing_files() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("gone.py");

        let snapshot = InputSnapshot::capture([abs(&missing)], None, None, None).await;
        assert!(snapshot.is_empty());
    }

    #[tokio::test]
    async fn capture_records_files_modified_after_the_cutoff_as_unconfirmed() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        let cutoff = mtime(&file) - Duration::from_secs(10);
        let snapshot = InputSnapshot::capture([abs(&file)], Some(cutoff), None, None).await;
        assert!(matches!(
            snapshot.get(&file),
            Some(InputFileState::Unconfirmed(_))
        ));

        // An unconfirmed state cannot vouch for the cached outputs.
        assert!(matches!(
            snapshot.verify([abs(&file)], Some(cutoff), None).await,
            SnapshotFreshness::Stale(_)
        ));
    }

    /// The two-observation promotion: content observed before this build
    /// started and unchanged at its capture is what the build read.
    #[tokio::test]
    async fn capture_promotes_an_unconfirmed_fingerprint_seen_twice() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();
        set_future_mtime(&file);

        let first = InputSnapshot::capture([abs(&file)], Some(SystemTime::now()), None, None).await;
        assert!(matches!(
            first.get(&file),
            Some(InputFileState::Unconfirmed(_))
        ));

        let second =
            InputSnapshot::capture([abs(&file)], Some(SystemTime::now()), Some(&first), None).await;
        assert!(matches!(
            second.get(&file),
            Some(InputFileState::Fingerprint(_))
        ));
        assert!(matches!(
            second.verify([abs(&file)], None, None).await,
            SnapshotFreshness::Fresh
        ));
    }

    #[tokio::test]
    async fn capture_does_not_promote_changed_content() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"old").unwrap();
        set_future_mtime(&file);

        let first = InputSnapshot::capture([abs(&file)], Some(SystemTime::now()), None, None).await;
        fs_err::write(&file, b"new").unwrap();
        set_future_mtime(&file);
        let second =
            InputSnapshot::capture([abs(&file)], Some(SystemTime::now()), Some(&first), None).await;

        assert!(matches!(
            second.get(&file),
            Some(InputFileState::Unconfirmed(_))
        ));
    }

    /// A previous observation taken after this build started cannot bracket
    /// the build's reads: another process's capture must not confirm content
    /// this build may never have seen.
    #[tokio::test]
    async fn capture_does_not_promote_an_observation_from_during_the_build() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();
        set_future_mtime(&file);

        let build_started = SystemTime::now();
        let concurrent =
            InputSnapshot::capture([abs(&file)], Some(SystemTime::now()), None, None).await;
        let captured =
            InputSnapshot::capture([abs(&file)], Some(build_started), Some(&concurrent), None)
                .await;

        assert!(
            matches!(captured.get(&file), Some(InputFileState::Unconfirmed(_))),
            "an observation from during the build must not promote",
        );
    }

    #[tokio::test]
    async fn verify_is_fresh_for_unchanged_files() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        let snapshot = InputSnapshot::capture([abs(&file)], None, None, None).await;
        assert!(matches!(
            snapshot.verify([abs(&file)], None, None).await,
            SnapshotFreshness::Fresh
        ));
    }

    #[tokio::test]
    async fn verify_refreshes_a_touched_but_unchanged_file() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        let mut snapshot = InputSnapshot::capture([abs(&file)], None, None, None).await;
        let touched = mtime(&file) + Duration::from_secs(1);
        set_modified(&file, touched);

        match snapshot.verify([abs(&file)], None, None).await {
            SnapshotFreshness::Refreshed(refreshed) => {
                snapshot.apply_refresh(refreshed);
                assert_eq!(snapshot.get(&file).unwrap().modified(), touched);
            }
            other => panic!("expected a refresh, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn verify_is_stale_for_a_same_size_edit() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"old").unwrap();

        let snapshot = InputSnapshot::capture([abs(&file)], None, None, None).await;
        let touched = mtime(&file) + Duration::from_secs(1);
        fs_err::write(&file, b"new").unwrap();
        set_modified(&file, touched);

        assert!(matches!(
            snapshot.verify([abs(&file)], None, None).await,
            SnapshotFreshness::Stale(_)
        ));
    }

    #[tokio::test]
    async fn verify_is_stale_for_a_deleted_file() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        let snapshot = InputSnapshot::capture([abs(&file)], None, None, None).await;
        fs_err::remove_file(&file).unwrap();

        assert!(matches!(
            snapshot.verify([abs(&file)], None, None).await,
            SnapshotFreshness::Stale(_)
        ));
    }

    #[tokio::test]
    async fn verify_is_stale_when_an_mtime_only_file_is_touched() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("assets");
        fs_err::create_dir_all(&dir).unwrap();

        let snapshot = InputSnapshot::capture([abs(&dir)], None, None, None).await;
        assert!(matches!(
            snapshot.verify([abs(&dir)], None, None).await,
            SnapshotFreshness::Fresh
        ));

        // A recorded mtime that no longer matches must invalidate; an
        // mtime-only file can never be re-verified through a hash.
        let touched = InputFileState::MtimeOnly(mtime(&dir) + Duration::from_secs(1));
        let snapshot = InputSnapshot::from_iter([(abs(&dir), touched)]);
        assert!(matches!(
            snapshot.verify([abs(&dir)], None, None).await,
            SnapshotFreshness::Stale(_)
        ));
    }

    /// A file the snapshot has no state for and no cutoff to compare against
    /// is reported as untracked, not as a modification from a time to itself.
    #[tokio::test]
    async fn verify_reports_a_stateless_file_without_cutoff_as_untracked() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        match InputSnapshot::default()
            .verify([abs(&file)], None, None)
            .await
        {
            SnapshotFreshness::Stale(stale) => {
                assert!(matches!(stale.reason, StaleFileReason::Untracked))
            }
            other => panic!("expected a stale result, got {other:?}"),
        }
    }

    /// A file that exists but cannot be hashed is unreadable, not removed.
    #[cfg(unix)]
    #[tokio::test]
    async fn verify_reports_a_permission_denied_file_as_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let file = temp.path().join("main.py");
        fs_err::write(&file, b"body").unwrap();

        let snapshot = InputSnapshot::capture([abs(&file)], None, None, None).await;
        set_modified(&file, mtime(&file) + Duration::from_secs(1));
        fs_err::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        if fs_err::File::open(&file).is_ok() {
            // Running as root; the permission bits cannot deny anything.
            return;
        }

        match snapshot.verify([abs(&file)], None, None).await {
            SnapshotFreshness::Stale(stale) => {
                assert!(matches!(stale.reason, StaleFileReason::Unreadable))
            }
            other => panic!("expected a stale result, got {other:?}"),
        }
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
            snapshot.verify([abs(&file)], Some(cutoff), None).await,
            SnapshotFreshness::Fresh
        ));

        assert!(matches!(
            snapshot.verify([abs(&file)], None, None).await,
            SnapshotFreshness::Stale(_)
        ));

        set_modified(&file, cutoff + Duration::from_secs(10));
        assert!(matches!(
            snapshot.verify([abs(&file)], Some(cutoff), None).await,
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
        set_future_mtime(&file);

        let first = InputSnapshot::capture([abs(&file)], None, None, None).await;
        assert!(matches!(
            first.get(&file),
            Some(InputFileState::Fingerprint(_))
        ));

        let second =
            InputSnapshot::capture([abs(&file)], Some(SystemTime::now()), Some(&first), None).await;
        assert!(
            matches!(second.get(&file), Some(InputFileState::Fingerprint(_))),
            "a trusted previous observation of the same content must promote",
        );

        // And the trust survives further captures that keep seeing it.
        let third =
            InputSnapshot::capture([abs(&file)], Some(SystemTime::now()), Some(&second), None)
                .await;
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

        let first = InputSnapshot::capture([abs(&path)], None, None, None).await;
        assert!(matches!(
            first.get(&path),
            Some(InputFileState::MtimeOnly(_))
        ));

        fs_err::remove_dir(&path).unwrap();
        fs_err::write(&path, b"body").unwrap();
        set_future_mtime(&path);

        let second =
            InputSnapshot::capture([abs(&path)], Some(SystemTime::now()), Some(&first), None).await;
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
        set_future_mtime(&seen);
        set_future_mtime(&fresh);

        let previous = InputSnapshot::capture([abs(&seen)], None, None, None).await;
        let captured = InputSnapshot::capture(
            [abs(&seen), abs(&fresh)],
            Some(SystemTime::now()),
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
        let snapshot = InputSnapshot::capture([abs(&file)], Some(cutoff), None, None).await;
        assert!(matches!(
            snapshot.get(&file),
            Some(InputFileState::Fingerprint(_))
        ));

        set_modified(&file, cutoff + Duration::from_secs(1));
        let snapshot = InputSnapshot::capture([abs(&file)], Some(cutoff), None, None).await;
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

        let snapshot = InputSnapshot::capture([abs(&dir)], Some(cutoff), None, None).await;
        assert!(
            matches!(snapshot.get(&dir), Some(InputFileState::MtimeOnly(_))),
            "dropping the input leaves it unvalidated forever, which is weaker \
             than any recorded state",
        );
    }
}
