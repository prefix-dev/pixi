use std::{
    fs::File,
    path::{Path, PathBuf},
};

use globwalk::{DirEntry, WalkError};
use itertools::Itertools;
use rattler_digest::{digest::Digest, Sha256, Sha256Hash};
use thiserror::Error;

/// InputHash is a struct that contains the hash of the input files and the
/// globs used to generate the hash.
#[derive(Debug, Clone, Default)]
pub struct InputHash {
    pub hash: Sha256Hash,
    pub globs: Vec<String>,
}

#[derive(Error, Debug)]
pub enum InputHashError {
    #[error(transparent)]
    GlobWalk(#[from] globwalk::GlobError),

    #[error("failed to access {}", .0.display())]
    IoError(PathBuf, #[source] std::io::Error),

    #[error("unexpected io error occurred while accessing {}", .0.display())]
    UnexpectedIoError(PathBuf),

    #[error(transparent)]
    WalkError(WalkError),

    #[error("the operation was cancelled")]
    Cancelled,
}

impl InputHash {
    pub fn from_globs(root_dir: &Path, globs: Vec<String>) -> Result<Self, InputHashError> {
        let mut entries = globwalk::GlobWalkerBuilder::from_patterns(root_dir, &globs)
            .build()?
            .map_ok(DirEntry::into_path)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                if let Some(path) = e.path().map(Path::to_path_buf) {
                    if let Some(io_error) = e.into_io_error() {
                        InputHashError::IoError(path, io_error)
                    } else {
                        InputHashError::UnexpectedIoError(path)
                    }
                } else {
                    InputHashError::WalkError(e)
                }
            })?;

        entries.sort();

        let mut hasher = Sha256::default();
        for entry in entries {
            // Construct a normalized file path to ensure consistent hashing across
            // platforms. And add it to the hash.
            let normalized_file_path = entry
                .strip_prefix(root_dir)
                .unwrap_or(&entry)
                .to_string_lossy()
                .replace("\\", "/");
            rattler_digest::digest::Update::update(&mut hasher, normalized_file_path.as_bytes());

            // Concatenate the contents of the file to the hash.
            File::open(&entry)
                .and_then(|mut f| std::io::copy(&mut f, &mut hasher))
                .map_err(|e| InputHashError::IoError(entry.clone(), e))?;
        }
        let hash = hasher.finalize();

        Ok(Self { hash, globs })
    }
}