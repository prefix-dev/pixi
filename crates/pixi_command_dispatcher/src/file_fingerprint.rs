use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Instant, SystemTime},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

/// A snapshot of a file used to validate source-build caches.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct FileFingerprint {
    pub(crate) modified: SystemTime,
    pub(crate) size: u64,
    pub(crate) hash: u64,
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
) -> Result<BTreeMap<PathBuf, FileFingerprint>, FileFingerprintError> {
    let paths = paths.into_iter().collect::<BTreeSet<_>>();
    let file_count = paths.len();
    let started = Instant::now();
    let fingerprints: BTreeMap<PathBuf, FileFingerprint> = tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|path| fingerprint_file(&path).map(|fingerprint| (path, fingerprint)))
            .collect::<Result<BTreeMap<_, _>, _>>()
    })
    .await
    .expect("file fingerprint task panicked")?;
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
    Ok(fingerprints)
}

fn fingerprint_file(path: &Path) -> Result<FileFingerprint, FileFingerprintError> {
    // Retry once when a file changes while it is being read. This prevents
    // associating a hash of an inconsistent read with the final metadata.
    for _ in 0..2 {
        let before = fs_err::metadata(path)
            .map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;
        let modified = before
            .modified()
            .map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;

        let file =
            File::open(path).map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;
        let mut reader = BufReader::new(file);
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

        let after = fs_err::metadata(path)
            .map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;
        let after_modified = after
            .modified()
            .map_err(|err| FileFingerprintError::new(path.to_path_buf(), err))?;
        if before.len() == after.len() && modified == after_modified {
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
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn content_hash_is_stable_when_only_mtime_changes() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("input.txt");
        fs_err::write(&path, b"same contents").unwrap();

        let first = fingerprint_files([path.clone()])
            .await
            .unwrap()
            .remove(&path)
            .unwrap();
        File::open(&path)
            .unwrap()
            .set_modified(first.modified + Duration::from_secs(1))
            .unwrap();
        let second = fingerprint_files([path.clone()])
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
        let first = fingerprint_files([path.clone()])
            .await
            .unwrap()
            .remove(&path)
            .unwrap();

        fs_err::write(&path, b"new").unwrap();
        let second = fingerprint_files([path.clone()])
            .await
            .unwrap()
            .remove(&path)
            .unwrap();

        assert_eq!(first.size, second.size);
        assert_ne!(first.hash, second.hash);
    }
}
