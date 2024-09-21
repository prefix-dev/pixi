use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

use itertools::{Either, Itertools};
use rattler_digest::{digest::Digest, Sha256, Sha256Hash};
use thiserror::Error;
use wax::Glob;

/// InputHash is a struct that contains the hash of the input files.
#[derive(Debug, Clone, Default)]
pub struct GlobHash {
    pub hash: Sha256Hash,
    #[cfg(test)]
    matching_files: Vec<String>,
}

#[derive(Error, Debug)]
pub enum GlobHashError {
    #[error("failed to access {}", .0.display())]
    IoError(PathBuf, #[source] io::Error),

    #[error(transparent)]
    GlobError(#[from] wax::BuildError),

    #[error(transparent)]
    WalkError(#[from] io::Error),

    #[error("the operation was cancelled")]
    Cancelled,
}

impl GlobHash {
    pub fn from_patterns<'a>(
        root_dir: &Path,
        globs: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, GlobHashError> {
        // If the root is not a directory or does not exist, return an empty map.
        if !root_dir.is_dir() {
            return Ok(Self::default());
        }

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
            .map(|g| Glob::new(g).map_err(GlobHashError::GlobError))
            .collect::<Result<Vec<_>, _>>()?;
        let exclusion_globs = exclusion_globs
            .into_iter()
            .map(|g| Glob::new(g).map_err(GlobHashError::GlobError))
            .collect::<Result<Vec<_>, _>>()?;

        let mut entries = inclusion_globs
            .iter()
            .flat_map(|glob| {
                glob.walk(root_dir)
                    .not(exclusion_globs.clone())
                    .expect("since the globs are already parsed this should not error")
            })
            .filter_map(|entry| {
                match entry {
                    Ok(entry) if entry.file_type().is_dir() => None,
                    Ok(entry) => Some(Ok(entry.into_path())),
                    Err(e) => {
                        let path = e.path().map(Path::to_path_buf);
                        let io_err = std::io::Error::from(e);
                        match io_err.kind() {
                            // Ignore DONE and permission errors
                            io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => None,
                            _ => Some(Err(if let Some(path) = path {
                                GlobHashError::IoError(path, io_err)
                            } else {
                                GlobHashError::WalkError(io_err)
                            })),
                        }
                    }
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        entries.sort();

        #[cfg(test)]
        let mut matching_files = Vec::new();

        let mut hasher = Sha256::default();
        for entry in entries {
            // Construct a normalized file path to ensure consistent hashing across
            // platforms. And add it to the hash.
            let relative_path = entry.strip_prefix(root_dir).unwrap_or(&entry);
            let normalized_file_path = relative_path.to_string_lossy().replace("\\", "/");
            rattler_digest::digest::Update::update(&mut hasher, normalized_file_path.as_bytes());

            #[cfg(test)]
            matching_files.push(normalized_file_path);

            // Concatenate the contents of the file to the hash.
            File::open(&entry)
                .and_then(|mut f| std::io::copy(&mut f, &mut hasher))
                .map_err(|e| GlobHashError::IoError(entry.clone(), e))?;
        }
        let hash = hasher.finalize();

        Ok(Self {
            hash,
            #[cfg(test)]
            matching_files,
        })
    }
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use itertools::Itertools;
    use rstest::*;

    #[fixture]
    pub fn testname() -> String {
        let thread_name = std::thread::current().name().unwrap().to_string();
        let test_name = thread_name.rsplit("::").next().unwrap_or(&thread_name);
        format!("glob_hash_{test_name}")
    }

    #[rstest]
    #[case::satisfiability(vec!["tests/satisfiability/source-dependency/**/*"])]
    #[case::satisfiability_ignore_lock(vec!["tests/satisfiability/source-dependency/**/*", "!tests/satisfiability/source-dependency/**/*.lock"])]
    #[case::non_glob(vec!["tests/satisfiability/source-dependency/pixi.toml"])]
    fn test_input_hash(testname: String, #[case] globs: Vec<&str>) {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let glob_hash = super::GlobHash::from_patterns(root_dir, globs.iter().copied()).unwrap();
        let snapshot = format!(
            "Globs:\n{}\nHash: {:x}\nMatched files:\n{}",
            globs
                .iter()
                .format_with("\n", |glob, f| f(&format_args!("- {}", glob))),
            glob_hash.hash,
            glob_hash
                .matching_files
                .iter()
                .format_with("\n", |glob, f| f(&format_args!("- {}", glob)))
        );
        insta::assert_snapshot!(testname, snapshot);
    }
}
