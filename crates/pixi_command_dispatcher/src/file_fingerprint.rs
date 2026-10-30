//! Content fingerprinting of single files: the [`FileFingerprint`] data type
//! and the blocking hash primitive used by
//! [`crate::input_snapshot::InputSnapshot`].

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use same_file::Handle;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Semaphore;
use xxhash_rust::xxh3::Xxh3;

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

/// A content snapshot of a file: size and mtime for cheap comparisons, an
/// XXH3-64 hash to prove contents unchanged when only the mtime moved.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct FileFingerprint {
    #[serde(with = "system_time_serde")]
    pub(crate) modified: SystemTime,
    pub(crate) size: u64,
    pub(crate) hash: u64,
}

/// Persists filesystem timestamps through `chrono`, which round-trips
/// pre-epoch instants and nanosecond precision.
pub(crate) mod system_time_serde {
    use std::time::SystemTime;

    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        DateTime::<Utc>::from(*time).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        DateTime::<Utc>::deserialize(deserializer).map(SystemTime::from)
    }

    pub(crate) mod optional {
        use std::time::SystemTime;

        use chrono::{DateTime, Utc};
        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S>(time: &Option<SystemTime>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match time {
                Some(time) => super::serialize(time, serializer),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<SystemTime>, D::Error>
        where
            D: Deserializer<'de>,
        {
            Ok(Option::<DateTime<Utc>>::deserialize(deserializer)?.map(SystemTime::from))
        }
    }
}

/// Hashes a single regular file, retrying once when it changes mid-read so a
/// hash of an inconsistent read is never paired with the final metadata.
pub(crate) fn fingerprint_file(path: &Path) -> Result<FileFingerprint, FileFingerprintError> {
    fingerprint_file_inner(path).map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))
}

fn fingerprint_file_inner(path: &Path) -> std::io::Result<FileFingerprint> {
    fn not_a_regular_file() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file")
    }

    // Opening a special file can block indefinitely (e.g. a FIFO without a
    // writer), so check the file type from a stat before opening.
    if !fs_err::metadata(path)?.file_type().is_file() {
        return Err(not_a_regular_file());
    }

    for _ in 0..2 {
        let handle = Handle::from_file(File::open(path)?)?;
        let before = handle.as_file().metadata()?;
        if !before.file_type().is_file() {
            return Err(not_a_regular_file());
        }
        let modified = before.modified()?;

        let hash = hash_file_contents(handle.as_file())?;

        let after = handle.as_file().metadata()?;
        let path_unchanged = handle == Handle::from_path(path)?;
        if path_unchanged && before.len() == after.len() && modified == after.modified()? {
            return Ok(FileFingerprint {
                modified,
                size: after.len(),
                hash,
            });
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "file changed while computing its fingerprint",
    ))
}

/// XXH3-64 over everything `reader` yields.
pub(crate) fn hash_file_contents(mut reader: impl Read) -> std::io::Result<u64> {
    let mut hasher = Xxh3::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.digest())
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
pub(crate) mod test_helpers {
    use std::{fs::OpenOptions, path::Path, time::SystemTime};

    pub(crate) fn set_modified(path: &Path, modified: SystemTime) {
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    pub(crate) fn mtime(path: &Path) -> SystemTime {
        fs_err::metadata(path).unwrap().modified().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use tempfile::TempDir;

    use super::{test_helpers::set_modified, *};

    #[test]
    fn content_hash_is_stable_when_only_mtime_changes() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("input.txt");
        fs_err::write(&path, b"same contents").unwrap();

        let first = fingerprint_file(&path).unwrap();
        set_modified(&path, first.modified + Duration::from_secs(1));
        let second = fingerprint_file(&path).unwrap();

        assert_ne!(first.modified, second.modified);
        assert_eq!(first.size, second.size);
        assert_eq!(first.hash, second.hash);
    }

    #[test]
    fn content_hash_changes_for_same_size_edit() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("input.txt");
        fs_err::write(&path, b"old").unwrap();
        let first = fingerprint_file(&path).unwrap();

        fs_err::write(&path, b"new").unwrap();
        let second = fingerprint_file(&path).unwrap();

        assert_eq!(first.size, second.size);
        assert_ne!(first.hash, second.hash);
    }

    /// Two empty files hash identically; only size and path separate them.
    /// Confirms the hasher handles a zero-length read loop.
    #[test]
    fn empty_files_hash_consistently() {
        let temp = TempDir::new().unwrap();
        let a = temp.path().join("a.txt");
        let b = temp.path().join("b.txt");
        fs_err::write(&a, b"").unwrap();
        fs_err::write(&b, b"").unwrap();

        let first = fingerprint_file(&a).unwrap();
        let second = fingerprint_file(&b).unwrap();
        assert_eq!(first.size, 0);
        assert_eq!(first.hash, second.hash);
    }

    /// A directory matched by an input glob must be rejected from the stat
    /// alone, before anything is opened.
    #[test]
    fn directories_are_rejected_without_opening() {
        let temp = TempDir::new().unwrap();
        let err = fingerprint_file(temp.path()).unwrap_err();
        assert_eq!(err.source.kind(), std::io::ErrorKind::InvalidInput);
    }

    /// A FIFO would block forever if opened without a writer, so the file
    /// type check must reject it from the stat alone.
    #[cfg(unix)]
    #[test]
    fn fifos_are_rejected_without_opening_them() {
        use std::os::unix::fs::FileTypeExt;

        let temp = TempDir::new().unwrap();
        let fifo = temp.path().join("pipe");
        let status = std::process::Command::new("mkfifo").arg(&fifo).status();
        let Ok(status) = status else { return };
        if !status.success() {
            return;
        }
        assert!(fs_err::metadata(&fifo).unwrap().file_type().is_fifo());

        let err = fingerprint_file(&fifo).unwrap_err();
        assert_eq!(err.source.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn pre_epoch_mtime_round_trips_through_json() {
        let fingerprint = FileFingerprint {
            modified: UNIX_EPOCH - Duration::from_millis(1_500),
            size: 3,
            hash: 42,
        };

        let json = serde_json::to_string(&fingerprint).unwrap();
        assert!(json.contains("1969-12-31T23:59:58.500"), "got {json}");
        assert_eq!(
            serde_json::from_str::<FileFingerprint>(&json).unwrap(),
            fingerprint
        );
    }

    #[test]
    fn mtime_serde_keeps_nanosecond_precision() {
        let fingerprint = FileFingerprint {
            modified: UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789),
            size: 3,
            hash: 42,
        };

        let json = serde_json::to_string(&fingerprint).unwrap();
        assert_eq!(
            serde_json::from_str::<FileFingerprint>(&json).unwrap(),
            fingerprint
        );
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

        assert_ne!(Handle::from_path(&path).unwrap(), handle);
    }
}
