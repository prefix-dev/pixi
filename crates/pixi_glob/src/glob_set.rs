use std::{
    io,
    path::{Path, PathBuf},
};

use itertools::{Either, Itertools};
use thiserror::Error;
use wax::{Glob, WalkEntry};

/// A set of globs to include and exclude from a directory.
pub struct GlobSet<'t> {
    /// The globs to include in the filter.
    pub include: Vec<Glob<'t>>,
    /// The globs to exclude from the filter.
    pub exclude: Vec<Glob<'t>>,
}

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum GlobSetError {
    #[error("failed to access {}", .0.display())]
    Io(PathBuf, #[source] io::Error),

    #[error(transparent)]
    DirWalk(#[from] io::Error),

    #[error("failed to read metadata for {0}")]
    Metadata(PathBuf, #[source] wax::WalkError),

    #[error(transparent)]
    Build(#[from] wax::BuildError),

    #[error(transparent)]
    StripPrefix(#[from] std::path::StripPrefixError),
}

impl<'t> GlobSet<'t> {
    /// Create a new `GlobSet` from a list of globs.
    ///
    /// The globs are split into inclusion and exclusion globs based on whether they
    /// start with `!`.
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
            .map(Glob::new)
            .collect::<Result<Vec<_>, _>>()?;
        let exclusion_globs = exclusion_globs
            .into_iter()
            .map(Glob::new)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            include: inclusion_globs,
            exclude: exclusion_globs,
        })
    }

    /// Create a function that filters out files that match the globs.
    pub fn filter_directory(
        &'t self,
        root_dir: &Path,
    ) -> impl Iterator<Item = Result<WalkEntry<'static>, GlobSetError>> + 't {
        let root_dir = root_dir.to_path_buf();
        let entries = self
            .include
            .iter()
            .flat_map(move |glob| {
                let (effective_walk_root, glob) = if glob.has_semantic_literals() {
                    // if the glob has semantic literals, we need to
                    // join the root directory with the glob prefix
                    // and use that as the effective walk root.
                    // Example:
                    // if `root_dir` is "/path/to/src" and `glob` is "../**/*.cpp",
                    //   `effective_walk_root` becomes "/path/to".
                    let (prefix, glob) = glob.clone().partition();
                    (root_dir.join(&prefix), glob)
                } else {
                    (root_dir.clone(), glob.clone())
                };

                let walkable = glob
                    .walk(&effective_walk_root)
                    .not(self.exclude.clone())
                    .expect("since the globs are already parsed this should not error")
                    .collect_vec();

                walkable
                    .into_iter()
                    .map(|w| {
                        w.map_err(|e| GlobSetError::Metadata(effective_walk_root.to_path_buf(), e))
                    })
                    .collect_vec()
                    .into_iter()
            })
            .filter_map(|entry| {
                match entry {
                    Ok(entry) if entry.file_type().is_dir() => None,
                    Ok(entry) => Some(Ok(entry)),
                    Err(e) => {
                        match e {
                            GlobSetError::Metadata(_, we) => {
                                let path = we.path().map(Path::to_path_buf);
                                let io_err = std::io::Error::from(we);
                                match io_err.kind() {
                                    // Ignore DONE and permission errors
                                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => {
                                        None
                                    }
                                    _ => Some(Err(if let Some(path) = path {
                                        GlobSetError::Io(path, io_err)
                                    } else {
                                        GlobSetError::DirWalk(io_err)
                                    })),
                                }
                            }
                            _ => Some(Err(e)),
                        }
                    }
                }
            });
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::GlobSet;
    use fs_err::File;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn test_filter_globs_inclusion_exclusion() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();

        // Create files and directories
        File::create(root_path.join("include1.txt")).unwrap();
        File::create(root_path.join("include2.log")).unwrap();
        File::create(root_path.join("exclude.txt")).unwrap();
        fs_err::create_dir(root_path.join("subdir")).unwrap();
        File::create(root_path.join("subdir/include_subdir.txt")).unwrap();

        // Test globs: include all .txt but exclude exclude.txt
        let filter_globs = GlobSet::create(vec!["**/*.txt", "!exclude.txt"]).unwrap();

        // Filter directory and get results as strings
        let mut filtered_files: Vec<_> = filter_globs
            .filter_directory(root_path)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|p| p.path().strip_prefix(root_path).unwrap().to_path_buf())
            .collect();

        // Assert the expected files are present
        filtered_files.sort();

        let mut expected = vec![
            "include1.txt".parse::<PathBuf>().unwrap(),
            "subdir/include_subdir.txt".parse().unwrap(),
        ];
        expected.sort();
        assert_eq!(filtered_files, expected);
    }

    #[test]
    fn test_filters_with_relatives_globs() {
        // In this test we want to make sure that when globbing over
        // patterns that contains semantic relative path, like
        // ../pixi.toml or ../sources/*.toml, we are able to
        // distinguish between glob and just a semantic path.
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();

        let temp_path_as_root = temp_dir.path().join("somewhere_inside");
        fs_err::create_dir(&temp_path_as_root).unwrap();

        // Create files and directories
        fs_err::create_dir(root_path.join("subdir")).unwrap();
        File::create(root_path.join("subdir/some_inner_source.cpp")).unwrap();

        // Test globs: we want to get the file inside the subdir using a relative glob.
        let filter_globs = GlobSet::create(vec!["../**/*.cpp", "!exclude.txt"]).unwrap();

        // Filter directory and get results as strings
        let mut filtered_files: Vec<_> = filter_globs
            // pretend that we are in the workspace folder
            // and our recipe yaml is living inside some folder
            // that will point outside
            .filter_directory(&temp_path_as_root)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|p| {
                p.path()
                    .strip_prefix(&temp_path_as_root)
                    .unwrap()
                    .to_path_buf()
            })
            .collect();

        // Assert the expected files are present
        filtered_files.sort();

        let expected = vec![
            "../subdir/some_inner_source.cpp"
                .parse::<PathBuf>()
                .unwrap(),
        ];
        assert_eq!(filtered_files, expected);
    }

    #[test]
    fn test_filters_with_just_a_file_glob() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();

        // Create files and directories
        File::create(root_path.join("pixi.toml")).unwrap();

        // Test globs: include pixi.toml
        let filter_globs = GlobSet::create(vec!["pixi.toml"]).unwrap();

        // Filter directory and get results as strings
        let mut filtered_files: Vec<_> = filter_globs
            // pretend that we are in the workspace folder
            // and our recipe yaml is living inside some folder
            // that will point outside
            .filter_directory(root_path)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|p| p.path().strip_prefix(root_path).unwrap().to_path_buf())
            .collect();

        // Assert the expected files are present
        filtered_files.sort();

        let expected = vec!["pixi.toml".parse::<PathBuf>().unwrap()];
        assert_eq!(filtered_files, expected);
    }
}
