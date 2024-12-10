mod outdated;
mod package_identifier;
mod records_by_name;
mod reporter;
mod resolve;
mod satisfiability;
mod update;
mod utils;

use crate::Project;
use miette::{IntoDiagnostic, WrapErr};
pub(crate) use package_identifier::PypiPackageIdentifier;
use pixi_record::PixiRecord;
use rattler_lock::{LockFile, ParseCondaLockError, PypiPackageData, PypiPackageEnvironmentData};
pub(crate) use records_by_name::{PixiRecordsByName, PypiRecordsByName};
pub(crate) use resolve::{
    conda::resolve_conda, pypi::resolve_pypi, uv_resolution_context::UvResolutionContext,
};
pub use satisfiability::{
    verify_environment_satisfiability, verify_platform_satisfiability, EnvironmentUnsat,
    PlatformUnsat,
};
pub(crate) use update::{LockFileDerivedData, UpdateContext};
pub use update::{UpdateLockFileOptions, UpdateMode};
pub(crate) use utils::filter_lock_file;

/// A list of conda packages that are locked for a specific platform.
pub type LockedCondaPackages = Vec<PixiRecord>;

/// A list of Pypi packages that are locked for a specific platform.
pub type LockedPypiPackages = Vec<PypiRecord>;

/// A single Pypi record that contains both the package data and the environment
/// data. In Pixi we basically always need both.
pub type PypiRecord = (PypiPackageData, PypiPackageEnvironmentData);

/// Loads the lockfile for the specified project or returns a dummy one if none
/// could be found.
pub async fn load_lock_file(project: &Project) -> miette::Result<LockFile> {
    let lock_file_path = project.lock_file_path();
    if lock_file_path.is_file() {
        // Spawn a background task because loading the file might be IO bound.
        tokio::task::spawn_blocking(move || {
            LockFile::from_path(&lock_file_path)
                .map_err(|err| match err {
                    ParseCondaLockError::IncompatibleVersion{ lock_file_version, max_supported_version} => {
                        miette::miette!(
                            help="Please update `pixi` version to the latest version and try again.",
                            "The lock file version is {}, but only up to including version {} is supported by the current version.",
                            lock_file_version, max_supported_version
                        )
                    }
                    _ => miette::miette!(err).into(),
                })
                .wrap_err_with(|| {
                    format!(
                        "Failed to load lock file from `{}`",
                        lock_file_path.display()
                    )
                })
        })
            .await
            .unwrap_or_else(|e| Err(e).into_diagnostic())
    } else {
        Ok(LockFile::default())
    }
}
