//! Implements the logic to very quickly compute the hashes of all files in a directory that match
//! a certain set of globs.
//!
//! Except for custom globs specified by the user, gitignore rules are respected. This means that
//! files that are ignored by git will also be ignored by logic defined in this module.
//!
//! The main entry-point to compute the hashes of all files in a directory is the
//! [`FileHashes::from_files`] method.

use ignore::{overrides::OverrideBuilder, WalkBuilder};
use itertools::Itertools;
use std::hash::Hash;
use std::{
    collections::HashMap,
    fs::File,
    hash::Hasher,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};
use thiserror::Error;
use tokio::task::JoinError;
use xxhash_rust::xxh3::Xxh3;

#[derive(Debug, Error)]
pub enum FileHashesError {
    #[error(transparent)]
    WalkError(#[from] ignore::Error),

    #[error("I/O error while reading file {0}")]
    IoError(PathBuf, #[source] std::io::Error),
}

/// A map of file paths to their hashes.
#[derive(Debug, Default)]
pub struct FileHashes {
    pub files: HashMap<PathBuf, String>,
}

impl Hash for FileHashes {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.files
            .iter()
            .sorted_by_key(|(path, _)| *path)
            .for_each(|(path, hash)| {
                path.hash(state);
                hash.hash(state);
            });
    }
}

impl FileHashes {
    /// Compute the hashes of the files that match the specified set of filters.
    ///
    /// Filters follow the same rules as gitignore rules. For example, `*.rs` will match all Rust
    /// files in the directory and `!src/lib.rs` will exclude the `src/lib.rs` file from the
    /// results.
    ///
    /// The `root` parameter specifies the directory in which the files are located. Only files
    /// are included in the result. Directories are not returned in the result but filtering on
    /// directories is supported.
    ///
    /// The hash is computed using the xxh3 algorithm which provides extremely fast hashing
    /// performance. The traversal, filtering and hash computations are also parallelized over all
    /// available CPU cores to maximize performance.
    pub async fn from_files(
        root: &Path,
        filters: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self, FileHashesError> {
        // If the root is not a directory or does not exist, return an empty map.
        if !root.is_dir() {
            return Ok(Self::default());
        }

        // Construct the custom filter
        let mut ignore_builder = OverrideBuilder::new(root);
        for ignore_line in filters {
            let path = root.join(ignore_line.as_ref());
            let mut pat = if ignore_line.as_ref().ends_with('/') {
                format!("{}**", ignore_line.as_ref())
            } else if path.exists() && path.is_dir() {
                format!("{}/**", ignore_line.as_ref())
            } else {
                ignore_line.as_ref().to_owned()
            };

            if pat.starts_with('!') && !pat.starts_with("!/") {
                // make sure there is a `/` at the 2nd place so that the pattern reads
                // `!/**/lib.rs` instead of `!**/lib.rs`
                pat.insert(1, '/');
            } else {
                // Same for the others, make sure they start in the right folder
                if !pat.starts_with('/') {
                    pat.insert(0, '/');
                }
            }
            ignore_builder.add(&pat)?;
        }

        let filter = ignore_builder.build()?;

        // Spawn a thread that will collect the results from a channel.
        let (tx, rx) = crossbeam_channel::bounded(100);
        let collect_handle =
            tokio::task::spawn_blocking(move || rx.iter().collect::<Result<HashMap<_, _>, _>>());

        // Iterate over all entries in parallel and send them over a channel to the collection thread.
        let collect_root = root.to_owned();
        WalkBuilder::new(root)
            .overrides(filter)
            .hidden(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .build_parallel()
            .run(|| {
                let tx = tx.clone();
                let collect_root = collect_root.clone();
                Box::new(move |entry| {
                    let result = match entry {
                        Ok(entry) if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) => {
                            return ignore::WalkState::Continue;
                        }
                        Ok(entry) => compute_file_hash(entry.path()).map(|hash| {
                            let path = entry
                                .path()
                                .strip_prefix(&collect_root)
                                .expect("path is not prefixed by the root");
                            tracing::info!("Added hash for file: {:?}", path);
                            (path.to_owned(), hash)
                        }),
                        Err(e) => Err(FileHashesError::from(e)),
                    };
                    match (result.is_err(), tx.send(result)) {
                        (true, _) => ignore::WalkState::Quit,
                        (_, Err(_)) => ignore::WalkState::Quit,
                        _ => ignore::WalkState::Continue,
                    }
                })
            });

        // Drop the local handle to the channel. This will close the channel which in turn will
        // cause the collection thread to finish which allows us to join without deadlocking.
        drop(tx);
        match collect_handle.await.map_err(JoinError::try_into_panic) {
            Ok(files) => Ok(Self { files: files? }),
            Err(Ok(panic)) => std::panic::resume_unwind(panic),
            Err(Err(_)) => panic!("the task was cancelled"),
        }
    }
}

/// Computes the xxh3 hash of a file.
fn compute_file_hash(path: &Path) -> Result<String, FileHashesError> {
    let mut file =
        BufReader::new(File::open(path).map_err(|e| FileHashesError::IoError(path.to_owned(), e))?);
    let mut hasher = Box::new(Xxh3::new());
    loop {
        let buf = file
            .fill_buf()
            .map_err(|e| FileHashesError::IoError(path.to_owned(), e))?;
        let bytes_read = buf.len();
        if bytes_read == 0 {
            break;
        }
        hasher.update(buf);
        file.consume(bytes_read);
    }

    Ok(format!("{:x}", hasher.finish()))
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;
    use std::fs::{create_dir, write};
    use tempfile::tempdir;

    #[tokio::test]
    async fn compute_hashes() {
        let target_dir = tempdir().unwrap();

        // Create a directory structure with a few files.
        create_dir(target_dir.path().join("src")).unwrap();
        create_dir(target_dir.path().join("src/bla")).unwrap();
        write(target_dir.path().join("build.rs"), "fn main() {}").unwrap();
        write(target_dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        write(target_dir.path().join("src/lib.rs"), "fn main() {}").unwrap();
        write(target_dir.path().join("src/bla/lib.rs"), "fn main() {}").unwrap();
        write(target_dir.path().join("Cargo.toml"), "[package]").unwrap();

        // Compute the hashes of all files in the directory that match a certain set of includes.
        let hashes =
            FileHashes::from_files(target_dir.path(), vec!["src/*.rs", "*.toml", "!**/lib.rs"])
                .await
                .unwrap();

        assert!(
            !hashes.files.contains_key(Path::new("build.rs")),
            "build.rs should not be included"
        );
        assert!(
            !hashes.files.contains_key(Path::new("src/lib.rs")),
            "lib.rs should not be included"
        );
        assert_matches!(
            hashes
                .files
                .get(Path::new("Cargo.toml"))
                .map(String::as_str),
            Some("e2513d27f6226691")
        );
        assert_matches!(
            hashes
                .files
                .get(Path::new("src/main.rs"))
                .map(String::as_str),
            Some("2c806b6ebece677c")
        );

        #[cfg(unix)]
        {
            let mut hasher = Xxh3::new();
            hashes.hash(&mut hasher);
            let s = format!("{:x}", hasher.finish());
            assert_eq!(s, "be05bb5d7c6e8e6");
        }

        let hashes = FileHashes::from_files(target_dir.path(), vec!["src/"])
            .await
            .unwrap();

        assert!(hashes.files.contains_key(Path::new("src/main.rs")));
        assert!(hashes.files.contains_key(Path::new("src/lib.rs")));
        assert!(hashes.files.contains_key(Path::new("src/bla/lib.rs")));
        assert!(!hashes.files.contains_key(Path::new("Cargo.toml")));

        // make sure that this also works without the trailing `/`
        let hashes = FileHashes::from_files(target_dir.path(), vec!["src"])
            .await
            .unwrap();

        assert!(hashes.files.contains_key(Path::new("src/main.rs")));
        assert!(hashes.files.contains_key(Path::new("src/lib.rs")));
        assert!(hashes.files.contains_key(Path::new("src/bla/lib.rs")));
        assert!(!hashes.files.contains_key(Path::new("Cargo.toml")));

        let hashes = FileHashes::from_files(target_dir.path(), vec!["main.rs"])
            .await
            .unwrap();

        assert!(!hashes.files.contains_key(Path::new("src/main.rs")));

        let hashes = FileHashes::from_files(target_dir.path(), vec!["src/lib.rs", "src/*.rs"])
            .await
            .unwrap();

        assert!(hashes.files.contains_key(Path::new("src/lib.rs")));
    }
}
