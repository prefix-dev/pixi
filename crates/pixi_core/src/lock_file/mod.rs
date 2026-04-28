mod install_subset;
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
pub use install_subset::{FilteredPackages, InstallSubset};
pub use package_identifier::PypiPackageIdentifier;
use pixi_install_pypi::LockedPypiRecord;
use pixi_record::PixiRecord;
pub use pixi_uv_context::UvResolutionContext;
pub use rattler_lock::Verbatim;
pub use records_by_name::{
    HasNameVersion, PixiRecordsByName, PypiRecordsByName, UnresolvedPixiRecordsByName,
};
pub use resolve::pypi::resolve_pypi;
pub use satisfiability::{
    Dependency, EnvironmentUnsat, PlatformUnsat, resolve_dev_dependencies,
    verify_environment_satisfiability, verify_platform_satisfiability,
};
pub use update::{
    LockFileDerivedData, PackageFilterNames, ReinstallEnvironment, ReinstallPackages,
    SolveCondaEnvironmentError, UpdateContext, UpdateLockFileOptions, UpdateMode, UpdatedPrefix,
};
pub use utils::filter_lock_file;

pub use utils::IoConcurrencyLimit;

/// A list of conda packages that are locked for a specific platform.
pub type LockedCondaPackages = Vec<PixiRecord>;

/// A list of Pypi packages that are locked for a specific platform.
pub type LockedPypiRecords = Vec<LockedPypiRecord>;

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use pep440_rs::VersionSpecifiers;
    use pep508_rs::Requirement;
    use rattler_lock::{
        PackageHashes, PypiDistributionData, PypiPackageData, PypiSourceData, SourceData,
        UrlOrPath, Verbatim,
    };

    use crate::Workspace;

    pub fn make_wheel_package(name: &str, version: &str) -> PypiPackageData {
        make_wheel_package_with(
            name,
            version,
            Verbatim::new(UrlOrPath::Path(format!("./{name}").into())),
            None,
            None,
            vec![],
            None,
        )
    }

    pub fn make_source_package(name: &str) -> PypiPackageData {
        make_source_package_with(
            name,
            Verbatim::new(UrlOrPath::Path(format!("./{name}").into())),
            vec![],
            None,
        )
    }

    pub fn make_wheel_package_with(
        name: &str,
        version: &str,
        location: Verbatim<UrlOrPath>,
        hash: Option<PackageHashes>,
        index_url: Option<url::Url>,
        requires_dist: Vec<Requirement>,
        requires_python: Option<VersionSpecifiers>,
    ) -> PypiPackageData {
        PypiPackageData::Distribution(Box::new(PypiDistributionData {
            name: name.parse().unwrap(),
            version: pep440_rs::Version::from_str(version).unwrap(),
            location,
            hash,
            index_url,
            requires_dist,
            requires_python,
        }))
    }

    pub fn make_source_package_with(
        name: &str,
        location: Verbatim<UrlOrPath>,
        requires_dist: Vec<Requirement>,
        requires_python: Option<VersionSpecifiers>,
    ) -> PypiPackageData {
        PypiPackageData::Source(Box::new(PypiSourceData {
            name: name.parse().unwrap(),
            location,
            requires_dist,
            requires_python,
            source_data: SourceData::default(),
        }))
    }

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

        let result = workspace.load_lock_file().await.unwrap();
        // Test that we get a VersionMismatch result
        match result {
            crate::lock_file::update::LockFileLoadResult::VersionMismatch {
                lock_file_version,
                max_supported_version: _,
            } => {
                assert_eq!(lock_file_version, 9999);
                // We got the version mismatch as expected
            }
            _ => panic!("Expected VersionMismatch, got {result:?}"),
        }
    }
}
