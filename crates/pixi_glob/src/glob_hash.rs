use std::{
    fs::File,
    io,
    io::{BufRead, Read, Write},
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
                .and_then(|mut file| normalize_line_endings(&mut file, &mut hasher))
                .map_err(move |e| GlobHashError::IoError(entry, e))?;
        }
        let hash = hasher.finalize();

        Ok(Self {
            hash,
            #[cfg(test)]
            matching_files,
        })
    }
}

/// This function copy the contents of the reader to the writer but normalizes
/// the line endings (e.g. replaces `\r\n` with `\n`) in text files.
fn normalize_line_endings<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<()> {
    let mut reader = io::BufReader::new(reader);

    // Get the first few bytes of the file and check if there is a `0x0` byte in the
    // input.
    let mut buffer = reader.fill_buf()?;
    if buffer.contains(&0) {
        // This file is binary, compute the hash varbatim.
        std::io::copy(&mut reader, writer)?;
    } else {
        // Read the contents of the file but ignore any `\r` characters.
        let mut last_cr_pos = None;
        let mut offset = 0;
        while !buffer.is_empty() {
            match memchr::memchr2(b'\r', b'\n', buffer) {
                Some(pos) if buffer[pos] == b'\r' => {
                    if last_cr_pos.is_some() {
                        // We previously detected a `\r` character but did not encounter a newline
                        writer.write_all(&[b'\r'])?;
                    }

                    // Process everything up to the '\r' character. Effectively ignoring it.
                    writer.write_all(&buffer[..pos])?;
                    reader.consume(pos + 1);
                    offset += pos + 1;
                    last_cr_pos = Some(pos + offset);
                }
                Some(pos) => {
                    // Encountered a newline character. If the last time we encountered the `\r` was
                    // not the previous character, we have to process the last
                    // `\r` character.
                    match last_cr_pos {
                        Some(last_cr_pos) if last_cr_pos + 1 == pos + offset => {
                            writer.write_all(&[b'\r'])?;
                            offset += pos + 1;
                        }
                        _ => last_cr_pos = None,
                    }

                    // Process everything up-to and including the newline character.
                    writer.write_all(&buffer[..=pos])?;
                    reader.consume(pos + 1);
                    offset += pos + 1;
                }
                None => {
                    if last_cr_pos.is_some() {
                        // We previously detected a `\r` character but did not encounter a newline
                        writer.write_all(&[b'\r'])?;
                        last_cr_pos = None;
                    }

                    // This batch of data does not contain any `\r` or `\n` characters. Process the
                    // entire chunk.
                    writer.write_all(buffer)?;
                    let buffer_len = buffer.len();
                    reader.consume(buffer_len);
                    offset += buffer_len
                }
            }
            buffer = reader.fill_buf()?;
        }

        if last_cr_pos.is_some() {
            // We detected a `\r` at the end of the input.
            writer.write_all(&[b'\r'])?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use itertools::Itertools;
    use rstest::*;

    use super::*;

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
        let glob_hash = GlobHash::from_patterns(root_dir, globs.iter().copied()).unwrap();
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

    #[test]
    fn test_normalize_line_endings() {
        let input =
            "\rHello\r\nWorld\r\nYou are the best\nThere is no-one\r\r \rlike you.\r".repeat(8196);
        let mut normalized: Vec<u8> = Vec::new();
        normalize_line_endings(&mut input.as_bytes(), &mut normalized).unwrap();
        let output = String::from_utf8(normalized).unwrap();
        assert_eq!(output, input.replace("\r\n", "\n"));
    }
}
