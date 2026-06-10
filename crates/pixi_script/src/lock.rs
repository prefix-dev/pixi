//! Sidecar lock files for scripts with inline metadata.
//!
//! `pixi exec --lock script.py` records the resolved environment of a script
//! in a `script.py.pixi.lock` file next to it, similar to
//! `conda exec --lock`. The lock file is a regular pixi/rattler lock document
//! prefixed with a comment that records a digest of the script's metadata
//! block:
//!
//! ```yaml
//! # pixi-script-input-hash: sha256:...
//! version: 6
//! ...
//! ```
//!
//! When the digest still matches the script's metadata, the environment is
//! created from the locked packages instead of solving again; when it does
//! not, the lock file is considered out of date.

use std::path::{Path, PathBuf};

use rattler_conda_types::{Platform, RepoDataRecord};
use rattler_lock::{CondaPackageData, LockFile, PlatformData, PlatformName};
use thiserror::Error;

/// The suffix appended to the script file name to form its lock file name.
pub const LOCK_FILE_SUFFIX: &str = ".pixi.lock";

/// The comment line that records the digest of the script metadata the lock
/// file was created from.
const INPUT_HASH_PREFIX: &str = "# pixi-script-input-hash: sha256:";

/// The single environment stored in a script lock file.
const ENVIRONMENT_NAME: &str = rattler_lock::DEFAULT_ENVIRONMENT_NAME;

/// Errors that can occur while reading or writing a script lock file.
#[derive(Debug, Error, miette::Diagnostic)]
pub enum ScriptLockError {
    #[error("failed to read script lock file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write script lock file {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse script lock file {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<rattler_lock::ParseCondaLockError>,
    },

    #[error("script lock file {path} does not contain a `{ENVIRONMENT_NAME}` environment")]
    MissingEnvironment { path: PathBuf },

    #[error("failed to convert a locked package from {path}")]
    Conversion {
        path: PathBuf,
        #[source]
        source: rattler_lock::ConversionError,
    },

    #[error("failed to build the lock file document")]
    Build(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// The path of the lock file belonging to `script`: the script's file name
/// with [`LOCK_FILE_SUFFIX`] appended, in the same directory.
pub fn lock_path(script: &Path) -> PathBuf {
    let mut file_name = script.file_name().unwrap_or_default().to_os_string();
    file_name.push(LOCK_FILE_SUFFIX);
    script.with_file_name(file_name)
}

/// The digest of a script's metadata document that ties a lock file to the
/// metadata it was created from.
pub fn input_hash(document: &str) -> String {
    let digest =
        rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(document.as_bytes());
    format!("{digest:x}")
}

/// A script's sidecar lock file.
pub struct ScriptLock {
    lock_file: LockFile,
    /// The metadata digest recorded in the file, when present.
    input_hash: Option<String>,
}

impl ScriptLock {
    /// Reads the lock file at `path`. Returns `Ok(None)` when it does not
    /// exist.
    pub fn read(path: &Path) -> Result<Option<Self>, ScriptLockError> {
        let source = match fs_err::read_to_string(path) {
            Ok(source) => source,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(ScriptLockError::Read {
                    path: path.to_path_buf(),
                    source: err,
                });
            }
        };

        let input_hash = source
            .lines()
            .take_while(|line| line.starts_with('#'))
            .find_map(|line| line.strip_prefix(INPUT_HASH_PREFIX))
            .map(|hash| hash.trim().to_string());

        let lock_file =
            LockFile::from_str_with_base_directory(&source, path.parent()).map_err(|source| {
                ScriptLockError::Parse {
                    path: path.to_path_buf(),
                    source: Box::new(source),
                }
            })?;

        Ok(Some(Self {
            lock_file,
            input_hash,
        }))
    }

    /// Whether the lock file was created from metadata with the given digest.
    pub fn is_up_to_date(&self, input_hash: &str) -> bool {
        self.input_hash.as_deref() == Some(input_hash)
    }

    /// The locked records for `platform`, or `None` when the lock file does
    /// not cover that platform.
    pub fn records(
        &self,
        platform: Platform,
        path: &Path,
    ) -> Result<Option<Vec<RepoDataRecord>>, ScriptLockError> {
        let environment = self
            .lock_file
            .environment(ENVIRONMENT_NAME)
            .ok_or_else(|| ScriptLockError::MissingEnvironment {
                path: path.to_path_buf(),
            })?;
        let Some(lock_platform) = self.lock_file.platform(platform.as_str()) else {
            return Ok(None);
        };
        environment
            .conda_repodata_records(lock_platform)
            .map_err(|source| ScriptLockError::Conversion {
                path: path.to_path_buf(),
                source,
            })
    }

    /// Writes a lock file for `records` solved on `platform` to `path`.
    ///
    /// When `previous` is an up-to-date lock for the same metadata, the
    /// records of its other platforms are carried over so that locking on one
    /// platform does not discard the resolutions of another.
    pub fn write(
        path: &Path,
        input_hash: &str,
        platform: Platform,
        records: &[RepoDataRecord],
        channels: impl IntoIterator<Item = String>,
        previous: Option<&ScriptLock>,
    ) -> Result<(), ScriptLockError> {
        let mut platform_records: Vec<(Platform, Vec<RepoDataRecord>)> = Vec::new();
        if let Some(previous) = previous {
            for lock_platform in previous.lock_file.platforms() {
                let subdir = lock_platform.subdir();
                if subdir == platform {
                    continue;
                }
                if let Some(records) = previous.records(subdir, path)? {
                    platform_records.push((subdir, records));
                }
            }
        }
        platform_records.push((platform, records.to_vec()));

        let platforms = platform_records
            .iter()
            .map(|(platform, _)| PlatformData {
                name: PlatformName::try_from(platform.as_str().to_string())
                    .expect("a conda subdir is a valid lock file platform name"),
                subdir: *platform,
                virtual_packages: Vec::new(),
            })
            .collect();

        let mut builder = LockFile::builder()
            .with_platforms(platforms)
            .map_err(|err| ScriptLockError::Build(err.into()))?;
        builder.set_channels(
            ENVIRONMENT_NAME,
            channels
                .into_iter()
                .map(rattler_lock::Channel::from)
                .collect::<Vec<_>>(),
        );
        for (platform, records) in platform_records {
            for record in records {
                builder
                    .add_conda_package(
                        ENVIRONMENT_NAME,
                        platform.as_str(),
                        CondaPackageData::from(record),
                    )
                    .map_err(|err| ScriptLockError::Build(err.into()))?;
            }
        }
        let lock_file = builder.finish();

        let document = lock_file
            .render_to_string()
            .map_err(|err| ScriptLockError::Build(err.into()))?;
        fs_err::write(path, format!("{INPUT_HASH_PREFIX}{input_hash}\n{document}")).map_err(
            |source| ScriptLockError::Write {
                path: path.to_path_buf(),
                source,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_conda_types::{PackageName, PackageRecord, Version};
    use url::Url;

    use super::*;

    fn record(name: &str, subdir: Platform) -> RepoDataRecord {
        let mut package_record = PackageRecord::new(
            PackageName::new_unchecked(name),
            Version::from_str("1.0").unwrap(),
            "0".to_string(),
        );
        package_record.subdir = subdir.as_str().to_string();
        let file_name = format!("{name}-1.0-0.conda");
        RepoDataRecord {
            url: Url::parse(&format!("https://example.com/{subdir}/{file_name}")).unwrap(),
            channel: Some("https://example.com".to_string()),
            identifier: rattler_conda_types::package::DistArchiveIdentifier::try_from_filename(
                &file_name,
            )
            .unwrap(),
            package_record,
        }
    }

    #[test]
    fn lock_path_appends_the_suffix() {
        assert_eq!(
            lock_path(Path::new("examples/fetch.py")),
            Path::new("examples/fetch.py.pixi.lock")
        );
    }

    #[test]
    fn input_hash_is_stable() {
        assert_eq!(input_hash("a"), input_hash("a"));
        assert_ne!(input_hash("a"), input_hash("b"));
    }

    #[test]
    fn write_read_roundtrip_preserves_other_platforms() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.py.pixi.lock");
        let hash = input_hash("[tool.conda]");

        // Lock for linux-64 first.
        ScriptLock::write(
            &path,
            &hash,
            Platform::Linux64,
            &[record("zlib", Platform::Linux64)],
            ["https://example.com/conda-forge".to_string()],
            None,
        )
        .unwrap();

        let lock = ScriptLock::read(&path).unwrap().unwrap();
        assert!(lock.is_up_to_date(&hash));
        assert!(!lock.is_up_to_date(&input_hash("something else")));
        let records = lock.records(Platform::Linux64, &path).unwrap().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].package_record.name.as_normalized(), "zlib");
        assert_eq!(lock.records(Platform::Win64, &path).unwrap(), None);

        // Locking win-64 on top keeps the linux-64 resolution.
        ScriptLock::write(
            &path,
            &hash,
            Platform::Win64,
            &[record("zlib", Platform::Win64)],
            ["https://example.com/conda-forge".to_string()],
            Some(&lock),
        )
        .unwrap();

        let lock = ScriptLock::read(&path).unwrap().unwrap();
        assert!(lock.records(Platform::Linux64, &path).unwrap().is_some());
        assert!(lock.records(Platform::Win64, &path).unwrap().is_some());

        // Re-locking a platform replaces its records instead of duplicating.
        ScriptLock::write(
            &path,
            &hash,
            Platform::Linux64,
            &[record("libzlib", Platform::Linux64)],
            ["https://example.com/conda-forge".to_string()],
            Some(&lock),
        )
        .unwrap();
        let lock = ScriptLock::read(&path).unwrap().unwrap();
        let records = lock.records(Platform::Linux64, &path).unwrap().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].package_record.name.as_normalized(), "libzlib");
    }

    #[test]
    fn missing_lock_file_reads_as_none() {
        assert!(
            ScriptLock::read(Path::new("/nonexistent/script.py.pixi.lock"))
                .unwrap()
                .is_none()
        );
    }
}
