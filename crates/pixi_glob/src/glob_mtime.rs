//! This module contains the `GlobModificationTime` struct which is used to calculate the newest modification time for the files that match the given glob patterns.
//! Use this if you want to find the newest modification time for a set of files that match a glob pattern.
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use thiserror::Error;

use crate::glob_set::{self, GlobSet};

/// Contains the newest modification time for the files that match the given glob patterns.
#[derive(Debug, Clone)]
pub struct GlobModificationTime {
    /// The newest modification time for the files that match the given glob patterns.
    pub modified_at: SystemTime,
    /// The designated file with the newest modification time.
    pub designated_file: PathBuf,
}

impl Default for GlobModificationTime {
    fn default() -> Self {
        Self {
            modified_at: SystemTime::UNIX_EPOCH,
            designated_file: PathBuf::new(),
        }
    }
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
        // If the root is not a directory or does not exist, return an empty map.
        if !root_dir.is_dir() {
            return Ok(Self::default());
        }

        let glob_set = GlobSet::create(globs)?;
        let entries: Vec<_> = glob_set.filter_directory(root_dir)?;

        #[cfg(test)]
        let mut matching_files = Vec::new();

        let mut latest = SystemTime::UNIX_EPOCH;
        let mut designated_file = PathBuf::new();

        // Find the newest modification time and the designated file
        for entry in entries {
            #[cfg(test)]
            matching_files.push(entry.matched_path.clone());

            let modified_entry = entry.metadata.modified().map_err(|e| {
                GlobModificationTimeError::CalculateMTime(entry.matched_path.clone(), e)
            })?;

            if latest >= modified_entry {
                continue;
            }

            latest = modified_entry;
            designated_file = entry.matched_path.clone();
        }
        Ok(Self {
            modified_at: latest,
            designated_file,
        })
    }

    /// Get the newest modification time.
    pub fn newest(&self) -> SystemTime {
        self.modified_at
    }

    /// Get the designated file with the newest modification time.
    pub fn designated_file(&self) -> &Path {
        &self.designated_file
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

        // Assert that the designated file is `file3.txt` with the latest modification time
        assert_eq!(glob_mod_time.designated_file(), dir_path.join("file3.txt"));
        assert_eq!(glob_mod_time.modified_at, now + Duration::from_secs(60));
    }
}
