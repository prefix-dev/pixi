use std::{
    io,
    path::{Path, PathBuf},
};

use itertools::{Either, Itertools};
use thiserror::Error;
use wax::Glob;

pub(crate) struct GlobSet<'t> {
    /// The globs to include in the filter.
    pub include: Vec<Glob<'t>>,
    /// The globs to exclude from the filter.
    pub exclude: Vec<Glob<'t>>,
}

#[derive(Error, Debug)]
pub enum GlobSetError {
    #[error("failed to access {}", .0.display())]
    IoError(PathBuf, #[source] io::Error),

    #[error(transparent)]
    WalkError(#[from] io::Error),

    #[error("failed to read metadata for {0}")]
    MetadataError(PathBuf, #[source] wax::WalkError),

    #[error(transparent)]
    BuildError(#[from] wax::BuildError),
}

pub(crate) struct MatchedFile {
    /// Path to the matched file
    pub matched_path: PathBuf,
    /// Metadata of the matched file
    pub metadata: std::fs::Metadata,
}

impl MatchedFile {
    pub fn new(matched_path: PathBuf, metadata: std::fs::Metadata) -> Self {
        Self {
            matched_path,
            metadata,
        }
    }
}

impl<'t> GlobSet<'t> {
    pub fn create(globs: impl IntoIterator<Item = &'t str>) -> Result<GlobSet<'t>, GlobSetError> {
        // Split the globs into inclusion and exclusion globs based on whether they
        // start with `!`.
        let (inclusion_globs, exclusion_globs): (Vec<_>, Vec<_>) =
            globs.into_iter().partition_map(|g| {
                g.strip_prefix("!")
                    .map(Either::Right)
                    .unwrap_or(Either::Left(g))
            });

        // Parse all globs
        let inclusion_globs = inclusion_globs
            .into_iter()
            .map(|g| Glob::new(g))
            .collect::<Result<Vec<_>, _>>()?;
        let exclusion_globs = exclusion_globs
            .into_iter()
            .map(|g| Glob::new(g))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            include: inclusion_globs,
            exclude: exclusion_globs,
        })
    }

    /// Create a function that filters out files that match the globs.
    pub fn filter_directory(&self, root_dir: &Path) -> Result<Vec<MatchedFile>, GlobSetError> {
        let entries = self
            .include
            .iter()
            .flat_map(|glob| {
                glob.walk(root_dir)
                    .not(self.exclude.clone())
                    .expect("since the globs are already parsed this should not error")
            })
            .filter_map(|entry| {
                match entry {
                    Ok(entry) if entry.file_type().is_dir() => None,
                    Ok(entry) => match entry.metadata() {
                        Err(e) => Some(Err(GlobSetError::MetadataError(entry.into_path(), e))),
                        Ok(metadata) => Some(Ok(MatchedFile::new(entry.into_path(), metadata))),
                    },
                    Err(e) => {
                        let path = e.path().map(Path::to_path_buf);
                        let io_err = std::io::Error::from(e);
                        match io_err.kind() {
                            // Ignore DONE and permission errors
                            io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => None,
                            _ => Some(Err(if let Some(path) = path {
                                GlobSetError::IoError(path, io_err)
                            } else {
                                GlobSetError::WalkError(io_err)
                            })),
                        }
                    }
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }
}
#[cfg(test)]

mod tests {
    use super::GlobSet;

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{
            fs::{create_dir, File},
            path::PathBuf,
        };
        use tempfile::tempdir;

        #[test]
        fn test_filter_globs_inclusion_exclusion() {
            let temp_dir = tempdir().unwrap();
            let root_path = temp_dir.path();

            // Create files and directories
            File::create(root_path.join("include1.txt")).unwrap();
            File::create(root_path.join("include2.log")).unwrap();
            File::create(root_path.join("exclude.txt")).unwrap();
            create_dir(root_path.join("subdir")).unwrap();
            File::create(root_path.join("subdir/include_subdir.txt")).unwrap();

            // Test globs: include all .txt but exclude exclude.txt
            let filter_globs = GlobSet::create(vec!["**/*.txt", "!exclude.txt"]).unwrap();

            // Filter directory and get results as strings
            let mut filtered_files: Vec<_> = filter_globs
                .filter_directory(&root_path)
                .unwrap()
                .into_iter()
                .map(|p| {
                    p.matched_path
                        .strip_prefix(&root_path)
                        .unwrap()
                        .to_path_buf()
                })
                .collect();

            // Assert the expected files are present
            assert_eq!(
                filtered_files.sort(),
                vec![
                    "include1.txt".parse::<PathBuf>().unwrap(),
                    "subdir/include_subdir.txt".parse().unwrap()
                ]
                .sort()
            );
        }
    }
}
