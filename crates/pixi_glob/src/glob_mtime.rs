use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use thiserror::Error;

use crate::glob_set::{self, GlobSet};

/// Contains the newest modification time for the files that match the given glob patterns.
#[derive(Debug, Clone)]
pub enum GlobModificationTime {
    /// No files matched the given glob patterns.
    NoMatches,
    /// Files matched the glob patterns, and this variant contains the newest modification time and designated file.
    MatchesFound {
        /// The newest modification time for the files that match the given glob patterns.
        modified_at: SystemTime,
        /// The designated file with the newest modification time.
        designated_file: PathBuf,
    },
}

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum GlobModificationTimeError {
    #[error("error calculating modification time for {}", .0.display())]
    CalculateMTime(PathBuf, #[source] std::io::Error),
    #[error(transparent)]
    GlobSet(#[from] glob_set::GlobSetError),
}

impl GlobModificationTime {
    /// Calculate the newest modification time for the files that match the given glob patterns.
    pub fn from_patterns<'a>(
        root_dir: &Path,
        globs: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, GlobModificationTimeError> {
        // If the root is not a directory or does not exist, return NoMatches.
        if !root_dir.is_dir() {
            return Ok(Self::NoMatches);
        }

        let glob_set = GlobSet::create(globs)?;
        let entries: Vec<_> = glob_set
            .filter_directory(root_dir)
            .collect::<Result<Vec<_>, _>>()?;

        let mut latest = None;
        let mut designated_file = PathBuf::new();

        // Find the newest modification time and the designated file
        for entry in entries {
            let matched_path = entry.path().to_owned();
            let metadata = entry.metadata().map_err(|e| {
                GlobModificationTimeError::CalculateMTime(matched_path.clone(), e.into())
            })?;
            let modified_entry = metadata
                .modified()
                .map_err(|e| GlobModificationTimeError::CalculateMTime(matched_path.clone(), e))?;

            if let Some(ref current_latest) = latest {
                if *current_latest >= modified_entry {
                    continue;
                }
            }

            latest = Some(modified_entry);
            designated_file = matched_path.clone();
        }

        match latest {
            Some(modified_at) => Ok(Self::MatchesFound {
                modified_at,
                designated_file,
            }),
            None => Ok(Self::NoMatches),
        }
    }

    /// Get the newest modification time, if any.
    pub fn newest(&self) -> Option<SystemTime> {
        match self {
            Self::MatchesFound { modified_at, .. } => Some(*modified_at),
            Self::NoMatches => None,
        }
    }

    /// Get the designated file with the newest modification time, if any.
    pub fn designated_file(&self) -> Option<&Path> {
        match self {
            Self::MatchesFound {
                designated_file, ..
            } => Some(designated_file.as_path()),
            Self::NoMatches => None,
        }
    }

    /// Returns `true` if there have been any matches found.
    pub fn has_matches(&self) -> bool {
        matches!(self, Self::MatchesFound { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::{Duration, SystemTime};
    use tempfile::tempdir;

    #[test]
    fn test_glob_modification_time() {
        // Create a temporary directory
        let temp_dir = tempdir().unwrap();
        let dir_path = temp_dir.path();

        // Two minutes ago
        let now = SystemTime::now() - Duration::from_secs(120);

        // Create files with different modification times
        let files = [
            // Three minutes ago
            ("file1.txt", now - Duration::from_secs(60)),
            // Two minutes ago
            ("file2.txt", now),
            // One minute ago <- should select this
            ("file3.txt", now + Duration::from_secs(60)),
        ];

        // Create files with different modification times
        for (name, mtime) in files {
            let path = dir_path.join(name);
            File::create(&path).unwrap().set_modified(mtime).unwrap();
        }

        // Use glob patterns to match `.txt` files
        let glob_mod_time = GlobModificationTime::from_patterns(dir_path, ["*.txt"]).unwrap();

        match glob_mod_time {
            GlobModificationTime::MatchesFound {
                modified_at,
                designated_file,
            } => {
                // Assert that the designated file is `file3.txt` with the latest modification time
                assert_eq!(designated_file, dir_path.join("file3.txt"));
                assert_eq!(modified_at, now + Duration::from_secs(60));
            }
            GlobModificationTime::NoMatches => panic!("Expected matches but found none"),
        }
    }

    #[test]
    fn test_glob_modification_time_no_matches() {
        // Create a temporary directory
        let temp_dir = tempdir().unwrap();
        let dir_path = temp_dir.path();

        // Use glob patterns that match no files
        let glob_mod_time = GlobModificationTime::from_patterns(dir_path, ["*.md"]).unwrap();

        assert!(matches!(glob_mod_time, GlobModificationTime::NoMatches));
        assert_eq!(glob_mod_time.newest(), None);
        assert_eq!(glob_mod_time.designated_file(), None);
    }
}
