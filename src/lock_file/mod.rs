mod outdated;
mod package_identifier;
mod records_by_name;
mod resolve;
mod satisfiability;
mod update;

use crate::Project;
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::RepoDataRecord;
use rattler_lock::{LockFile, PypiPackageData, PypiPackageEnvironmentData};

pub use outdated::OutdatedEnvironments;
pub use package_identifier::PypiPackageIdentifier;
pub use records_by_name::{PypiRecordsByName, RepoDataRecordsByName};
pub use resolve::{
    conda::resolve_conda, pypi::resolve_pypi, uv_resolution_context::UvResolutionContext,
};
pub use satisfiability::{verify_environment_satisfiability, verify_platform_satisfiability};
pub use update::{LockFileDerivedData, UpdateLockFileOptions};

/// A list of conda packages that are locked for a specific platform.
pub type LockedCondaPackages = Vec<RepoDataRecord>;

/// A list of Pypi packages that are locked for a specific platform.
pub type LockedPypiPackages = Vec<PypiRecord>;

/// A single Pypi record that contains both the package data and the environment data. In Pixi we
/// basically always need both.
pub type PypiRecord = (PypiPackageData, PypiPackageEnvironmentData);

/// Loads the lockfile for the specified project or returns a dummy one if none could be found.
pub async fn load_lock_file(project: &Project) -> miette::Result<LockFile> {
    let lock_file_path = project.lock_file_path();
    if lock_file_path.is_file() {
        // Spawn a background task because loading the file might be IO bound.
        tokio::task::spawn_blocking(move || {
            LockFile::from_path(&lock_file_path)
                .into_diagnostic()
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
