mod outdated;
mod package_identifier;
mod records_by_name;
mod reporter;
mod resolve;
mod satisfiability;
mod update;
mod utils;
pub mod virtual_packages;

pub use crate::environment::CondaPrefixUpdater;
pub use package_identifier::PypiPackageIdentifier;
use pixi_record::PixiRecord;
use rattler_lock::{PypiPackageData, PypiPackageEnvironmentData};
pub use records_by_name::{PixiRecordsByName, PypiRecordsByName};
pub use resolve::{pypi::resolve_pypi, uv_resolution_context::UvResolutionContext};
pub use satisfiability::{
    EnvironmentUnsat, PlatformUnsat, verify_environment_satisfiability,
    verify_platform_satisfiability,
};
pub use update::{LockFileDerivedData, ReinstallPackages, UpdateContext};
pub use update::{UpdateLockFileOptions, UpdateMode};
pub use utils::filter_lock_file;

pub use utils::IoConcurrencyLimit;

/// A list of conda packages that are locked for a specific platform.
pub type LockedCondaPackages = Vec<PixiRecord>;

/// A list of Pypi packages that are locked for a specific platform.
pub type LockedPypiPackages = Vec<PypiRecord>;

/// A single Pypi record that contains both the package data and the environment
/// data. In Pixi we basically always need both.
pub type PypiRecord = (PypiPackageData, PypiPackageEnvironmentData);

#[cfg(test)]
mod tests {
    use crate::Workspace;

    #[tokio::test]
    async fn test_load_newer_lock_file() {
        // Test that loading a lock file with a newer version than the current
        // version of pixi will return an error.
        let temp_dir = tempfile::tempdir().unwrap();
        let manifest_toml = r#"
        [project]
        name = "pixi"
        channels = []
        platforms = []
        "#;
        let workspace =
            Workspace::from_str(temp_dir.path().join("pixi.toml").as_path(), manifest_toml)
                .unwrap();

        let lock_file_path = workspace.lock_file_path();
        let raw_lock_file = r#"
        version: 9999
        environments:
        default:
            channels:
            - url: https://conda.anaconda.org/conda-forge/
            packages: {}
        packages: []
        "#;
        fs_err::tokio::write(&lock_file_path, raw_lock_file)
            .await
            .unwrap();

        let err = &workspace.load_lock_file().await.unwrap_err();
        let dbg_err = format!("{:?}", err);
        // Test that the error message contains the correct information.
        assert!(
            dbg_err.contains("The lock file version is 9999, but only up to including version")
        );
        // Also test that we try to help user by suggesting to update pixi.
        assert!(dbg_err.contains("Please update pixi to the latest version and try again."));
    }
}
