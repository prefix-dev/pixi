use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Instant, SystemTime},
};

use same_file::Handle;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Semaphore;
use xxhash_rust::xxh3::Xxh3;

use futures::{StreamExt, TryStreamExt};

/// Runs blocking filesystem work while holding a permit from the dispatcher's
/// shared I/O budget.
pub(crate) async fn spawn_blocking_with_io_permit<T>(
    io_semaphore: Option<Arc<Semaphore>>,
    task: impl FnOnce() -> T + Send + 'static,
) -> Result<T, tokio::task::JoinError>
where
    T: Send + 'static,
{
    let permit = match io_semaphore {
        Some(semaphore) => Some(
            semaphore
                .acquire_owned()
                .await
                .expect("I/O concurrency semaphore is never closed"),
        ),
        None => None,
    };
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        task()
    })
    .await
}

/// A snapshot of a file used to validate source-build caches.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct FileFingerprint {
    #[serde(with = "system_time_serde")]
    pub(crate) modified: SystemTime,
    pub(crate) size: u64,
    pub(crate) hash: u64,
}

mod system_time_serde {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

    #[derive(Serialize, Deserialize)]
    #[serde(untagged)]
    enum SystemTimeRepr {
        AfterEpoch {
            secs_since_epoch: u64,
            nanos_since_epoch: u32,
        },
        BeforeEpoch {
            secs_before_epoch: u64,
            nanos_before_epoch: u32,
        },
    }

    pub fn serialize<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let repr = match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => SystemTimeRepr::AfterEpoch {
                secs_since_epoch: duration.as_secs(),
                nanos_since_epoch: duration.subsec_nanos(),
            },
            Err(err) => {
                let duration = err.duration();
                SystemTimeRepr::BeforeEpoch {
                    secs_before_epoch: duration.as_secs(),
                    nanos_before_epoch: duration.subsec_nanos(),
                }
            }
        };
        repr.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (before_epoch, seconds, nanoseconds) = match SystemTimeRepr::deserialize(deserializer)?
        {
            SystemTimeRepr::AfterEpoch {
                secs_since_epoch,
                nanos_since_epoch,
            } => (false, secs_since_epoch, nanos_since_epoch),
            SystemTimeRepr::BeforeEpoch {
                secs_before_epoch,
                nanos_before_epoch,
            } => (true, secs_before_epoch, nanos_before_epoch),
        };
        if nanoseconds >= 1_000_000_000 {
            return Err(de::Error::custom(
                "system timestamp nanoseconds must be below one billion",
            ));
        }

        let duration = Duration::new(seconds, nanoseconds);
        if before_epoch {
            UNIX_EPOCH
                .checked_sub(duration)
                .ok_or_else(|| de::Error::custom("system timestamp is out of range"))
        } else {
            UNIX_EPOCH
                .checked_add(duration)
                .ok_or_else(|| de::Error::custom("system timestamp is out of range"))
        }
    }
}

impl FileFingerprint {
    /// Returns whether the metadata is identical, requires a content check, or
    /// already proves that the file changed.
    pub(crate) fn compare_metadata(
        &self,
        metadata: &std::fs::Metadata,
    ) -> Result<MetadataComparison, std::io::Error> {
        if metadata.len() != self.size {
            return Ok(MetadataComparison::Changed);
        }

        if metadata.modified()? == self.modified {
            Ok(MetadataComparison::Unchanged)
        } else {
            Ok(MetadataComparison::HashRequired)
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum MetadataComparison {
    Unchanged,
    HashRequired,
    Changed,
}

/// Computes fingerprints for all provided paths without blocking the async
/// executor.
pub(crate) async fn fingerprint_files(
    paths: impl IntoIterator<Item = PathBuf>,
    io_semaphore: Option<Arc<Semaphore>>,
) -> Result<BTreeMap<PathBuf, FileFingerprint>, FileFingerprintError> {
    let paths = paths.into_iter().collect::<BTreeSet<_>>();
    let file_count = paths.len();
    let started = Instant::now();
    // A permit represents one file read. This allows other users of the
    // dispatcher's shared I/O budget to make progress between files.
    let fingerprints = futures::stream::iter(paths)
        .map(|path| {
            let io_semaphore = io_semaphore.clone();
            async move {
                spawn_blocking_with_io_permit(io_semaphore, move || {
                    fingerprint_file(&path).map(|fingerprint| (path, fingerprint))
                })
                .await
                .expect("file fingerprint task panicked")
            }
        })
        .buffer_unordered(fingerprint_worker_count(file_count))
        .try_collect::<BTreeMap<_, _>>()
        .await?;
    log_fingerprint_stats(&fingerprints, file_count, started);
    Ok(fingerprints)
}

/// The result of fingerprinting a set of files while tolerating per-file
/// failures.
#[derive(Debug, Default)]
pub(crate) struct LenientFingerprints {
    /// Files that were hashed successfully.
    pub(crate) fingerprints: BTreeMap<PathBuf, FileFingerprint>,
    /// Files that could not be hashed but could still be stat'ed. These keep
    /// timestamp-based cache validation.
    pub(crate) mtime_only: BTreeMap<PathBuf, SystemTime>,
}

/// Computes fingerprints like [`fingerprint_files`] but never fails: a file
/// that cannot be hashed falls back to its mtime, and a file that cannot even
/// be stat'ed (e.g. deleted mid-build) is dropped entirely.
pub(crate) async fn fingerprint_files_lenient(
    paths: impl IntoIterator<Item = PathBuf>,
    io_semaphore: Option<Arc<Semaphore>>,
) -> LenientFingerprints {
    let paths = paths.into_iter().collect::<BTreeSet<_>>();
    let file_count = paths.len();
    let started = Instant::now();
    let outcomes = futures::stream::iter(paths)
        .map(|path| {
            let io_semaphore = io_semaphore.clone();
            async move {
                spawn_blocking_with_io_permit(io_semaphore, move || {
                    let outcome = match fingerprint_file(&path) {
                        Ok(fingerprint) => Ok(fingerprint),
                        Err(err) => {
                            let mtime =
                                fs_err::metadata(&path).and_then(|m| m.modified()).ok();
                            Err((err, mtime))
                        }
                    };
                    (path, outcome)
                })
                .await
                .expect("file fingerprint task panicked")
            }
        })
        .buffer_unordered(fingerprint_worker_count(file_count))
        .collect::<Vec<_>>()
        .await;

    let mut result = LenientFingerprints::default();
    for (path, outcome) in outcomes {
        match outcome {
            Ok(fingerprint) => {
                result.fingerprints.insert(path, fingerprint);
            }
            Err((err, Some(mtime))) => {
                tracing::debug!(
                    path = %path.display(),
                    error = %err,
                    "could not hash source input file; keeping timestamp-based validation"
                );
                result.mtime_only.insert(path, mtime);
            }
            Err((err, None)) => {
                tracing::debug!(
                    path = %path.display(),
                    error = %err,
                    "could not stat source input file; dropping it from the cache record"
                );
            }
        }
    }
    log_fingerprint_stats(&result.fingerprints, file_count, started);
    result
}

fn fingerprint_worker_count(file_count: usize) -> usize {
    file_count
        .min(
            std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
        )
        .max(1)
}

fn log_fingerprint_stats(
    fingerprints: &BTreeMap<PathBuf, FileFingerprint>,
    file_count: usize,
    started: Instant,
) {
    let total_bytes = fingerprints
        .values()
        .map(|fingerprint| fingerprint.size)
        .sum::<u64>();
    tracing::debug!(
        file_count,
        total_bytes,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "fingerprinted source-build input files"
    );
}

fn fingerprint_file(path: &Path) -> Result<FileFingerprint, FileFingerprintError> {
    // Opening a special file can block indefinitely (e.g. a FIFO without a
    // writer), so check the file type from a stat before opening.
    let file_type = fs_err::metadata(path)
        .map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?
        .file_type();
    if !file_type.is_file() {
        return Err(FileFingerprintError::new(
            path.to_path_buf(),
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
        ));
    }

    // Retry once when a file changes while it is being read. This prevents
    // associating a hash of an inconsistent read with the final metadata.
    for _ in 0..2 {
        let file =
            File::open(path).map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;
        let handle = Handle::from_file(file)
            .map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;
        let before = handle
            .as_file()
            .metadata()
            .map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;
        if !before.file_type().is_file() {
            return Err(FileFingerprintError::new(
                path.to_path_buf(),
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
            ));
        }
        let modified = before
            .modified()
            .map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;

        let mut reader = BufReader::new(handle.as_file());
        let mut hasher = Xxh3::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        drop(reader);

        let after = handle
            .as_file()
            .metadata()
            .map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;
        let after_modified = after
            .modified()
            .map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;
        let path_unchanged = path_matches_handle(path, &handle)
            .map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;
        if path_unchanged && before.len() == after.len() && modified == after_modified {
            return Ok(FileFingerprint {
                modified: after_modified,
                size: after.len(),
                hash: hasher.digest(),
            });
        }
    }

    Err(FileFingerprintError::new(
        path.to_path_buf(),
        std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "file changed while computing its fingerprint",
        ),
    ))
}

fn path_matches_handle(path: &Path, handle: &Handle) -> std::io::Result<bool> {
    Ok(*handle == Handle::from_path(path)?)
}

#[derive(Debug, Clone, Error)]
#[error("failed to fingerprint '{}'", path.display())]
pub(crate) struct FileFingerprintError {
    pub(crate) path: PathBuf,
    #[source]
    pub(crate) source: Arc<std::io::Error>,
}

impl FileFingerprintError {
    fn new(path: PathBuf, source: std::io::Error) -> Self {
        Self {
            path,
            source: Arc::new(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::OpenOptions,
        time::{Duration, UNIX_EPOCH},
    };

    use tempfile::TempDir;

    use super::*;

    fn set_modified(path: &Path, modified: SystemTime) {
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    #[tokio::test]
    async fn empty_input_produces_no_fingerprints() {
        let fingerprints = fingerprint_files([], Some(Arc::new(Semaphore::new(1))))
            .await
            .unwrap();
        assert!(fingerprints.is_empty());
    }

    #[tokio::test]
    async fn fingerprints_multiple_files_with_io_limit() {
        let temp = TempDir::new().unwrap();
        let paths = (0..20)
            .map(|index| {
                let path = temp.path().join(format!("{index}.txt"));
                fs_err::write(&path, index.to_string()).unwrap();
                path
            })
            .collect::<Vec<_>>();

        let fingerprints = fingerprint_files(paths, Some(Arc::new(Semaphore::new(2))))
            .await
            .unwrap();
        assert_eq!(fingerprints.len(), 20);
    }

    #[tokio::test]
    async fn content_hash_is_stable_when_only_mtime_changes() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("input.txt");
        fs_err::write(&path, b"same contents").unwrap();

        let first = fingerprint_files([path.clone()], None)
            .await
            .unwrap()
            .remove(&path)
            .unwrap();
        set_modified(&path, first.modified + Duration::from_secs(1));
        let second = fingerprint_files([path.clone()], None)
            .await
            .unwrap()
            .remove(&path)
            .unwrap();

        assert_ne!(first.modified, second.modified);
        assert_eq!(first.size, second.size);
        assert_eq!(first.hash, second.hash);
    }

    #[tokio::test]
    async fn content_hash_changes_for_same_size_edit() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("input.txt");
        fs_err::write(&path, b"old").unwrap();
        let first = fingerprint_files([path.clone()], None)
            .await
            .unwrap()
            .remove(&path)
            .unwrap();

        fs_err::write(&path, b"new").unwrap();
        let second = fingerprint_files([path.clone()], None)
            .await
            .unwrap()
            .remove(&path)
            .unwrap();

        assert_eq!(first.size, second.size);
        assert_ne!(first.hash, second.hash);
    }

    #[test]
    fn pre_epoch_mtime_round_trips_through_json() {
        let fingerprint = FileFingerprint {
            modified: UNIX_EPOCH - Duration::from_millis(1_500),
            size: 3,
            hash: 42,
        };

        let json = serde_json::to_string(&fingerprint).unwrap();
        assert!(json.contains("secs_before_epoch"));
        assert_eq!(
            serde_json::from_str::<FileFingerprint>(&json).unwrap(),
            fingerprint
        );
    }

    #[test]
    fn existing_positive_system_time_json_remains_readable() {
        let json = r#"{
            "modified": {
                "secs_since_epoch": 1,
                "nanos_since_epoch": 2
            },
            "size": 3,
            "hash": 42
        }"#;

        let fingerprint = serde_json::from_str::<FileFingerprint>(json).unwrap();
        assert_eq!(fingerprint.modified, UNIX_EPOCH + Duration::new(1, 2));
    }

    #[test]
    fn replacing_a_path_changes_its_file_identity() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("input.txt");
        let moved = temp.path().join("old-input.txt");
        fs_err::write(&path, b"old").unwrap();

        let handle = Handle::from_file(File::open(&path).unwrap()).unwrap();
        fs_err::rename(&path, moved).unwrap();
        fs_err::write(&path, b"new").unwrap();

        assert!(!path_matches_handle(&path, &handle).unwrap());
    }
}
