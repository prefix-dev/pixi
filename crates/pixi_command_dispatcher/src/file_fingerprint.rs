//! Content fingerprinting of single files: the [`FileFingerprint`] data type
//! and the blocking hash primitive used by
//! [`crate::input_snapshot::InputSnapshot`].

use std::{
    fs::File,
    io::{BufReader, Read},
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

pub(crate) mod system_time_serde {
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

/// Hashes a single regular file, retrying once when it changes mid-read so a
/// hash of an inconsistent read is never paired with the final metadata.
pub(crate) fn fingerprint_file(path: &Path) -> Result<FileFingerprint, FileFingerprintError> {
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
