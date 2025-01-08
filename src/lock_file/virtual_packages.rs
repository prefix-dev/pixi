use itertools::Itertools;
use miette::{Diagnostic, IntoDiagnostic};
use pixi_manifest::EnvironmentName;
use pypi_modifiers::pypi_tags::{
    get_tags_from_machine, is_python_record, tags_from_wheel_filename,
};
use rattler_conda_types::ParseStrictness::Lenient;
use rattler_conda_types::Platform;
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, Matches};
use rattler_lock::LockFile;
use rattler_virtual_packages::{
    DetectVirtualPackageError, VirtualPackage, VirtualPackageOverrides,
};
use std::collections::HashMap;
use std::str::FromStr;
use thiserror::Error;
use uv_distribution_filename::WheelFilename;
use uv_platform_tags::Tags;

#[derive(Debug, Error, Diagnostic)]
pub enum VirtualPackageError {
    #[error("Virtual package: {spec} not found on the system")]
    #[diagnostic(help("You can mock a virtual package by setting the override environment variable, e.g.: `CONDA_OVERRIDE_GLIBC=2.17`"))]
    VirtualPackageNotFound { spec: String },

    #[diagnostic(help("You can mock a virtual package by setting the override environment variable, e.g.: `CONDA_OVERRIDE_GLIBC=2.17`"))]
    #[error("Virtual package: {generic_virtual_pkg} does not match the required version: {spec}")]
    VirtualPackageVersionMismatch {
        generic_virtual_pkg: String,
        spec: String,
    },

    #[error("Couldn't get the virtual packages from the system")]
    VirtualPackageDetectionError(#[from] DetectVirtualPackageError),
}

/// Get the required virtual packages for the given environment based on the given lock file.
pub(crate) fn get_required_virtual_packages_from_conda_records(
    lock_file: &LockFile,
    platform: Platform,
    environment_name: &EnvironmentName,
) -> Vec<MatchSpec> {
    // Get the locked conda packages for the given platform and environment
    let locked_env = lock_file.environment(environment_name.as_str()).unwrap();
    let locked_records = locked_env
        .conda_repodata_records(platform)
        .unwrap()
        .collect_vec();

    locked_records
        .into_iter()
        .map(|record| record.package_record.depends)
        .flat_map(|deps| {
            deps.into_iter()
                .map(|dep| {
                    // Using lenient match spec to ignore issues in record
                    MatchSpec::from_str(dep.as_str(), Lenient).unwrap()
                })
                .filter(|dep| dep.is_virtual())
                .collect_vec()
        })
        .collect_vec()
}

fn all_wheel_tags(
    lock_file: &LockFile,
    platform: Platform,
    environment_name: &EnvironmentName,
) -> Vec<Tags> {
    // Get the locked conda packages for the given platform and environment
    let locked_pypi_data = lock_file
        .environment(environment_name.as_str())
        .and_then(|env| env.pypi_packages(platform))
        .unwrap()
        .cloned();

    locked_pypi_data
        .into_iter()
        .map(|package| package.0.location.clone())
        .flat_map(|location| {
            if let Some(file_name) = location.file_name() {
                let wheel = WheelFilename::from_str(file_name).unwrap();
                let tags = tags_from_wheel_filename(&wheel).unwrap();
                Some(tags)
            } else {
                None
            }
        })
        .collect_vec()
}
/// Validate that current machine has all the required virtual packages for the given environment
pub(crate) fn validate_virtual_packages(
    lock_file: &LockFile,
    platform: Platform,
    environment_name: &EnvironmentName,
    virtual_package_overrides: Option<VirtualPackageOverrides>,
) -> Result<bool, VirtualPackageError> {
    // Default to the environment variable overrides, but allow for an override for testing
    let virtual_package_overrides =
        virtual_package_overrides.unwrap_or(VirtualPackageOverrides::from_env());

    let required_virtual_packages =
        get_required_virtual_packages_from_conda_records(lock_file, platform, environment_name);

    // Get the virtual packages available on the system
    let system_virtual_packages = VirtualPackage::detect(&virtual_package_overrides)
        .unwrap()
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();

    // Check if all the required virtual conda packages match the system virtual packages
    for required in required_virtual_packages {
        if let Some(local_vpkg) = system_virtual_packages.get(
            &required
                .name
                .clone()
                .expect("Virtual package name not found"),
        ) {
            if required.matches(local_vpkg) {
                continue;
            } else {
                return Err(VirtualPackageError::VirtualPackageVersionMismatch {
                    generic_virtual_pkg: local_vpkg.clone().to_string(),
                    spec: required.clone().to_string(),
                });
            }
        } else {
            return Err(VirtualPackageError::VirtualPackageNotFound {
                spec: required.clone().to_string(),
            });
        }
    }

    // Get python record from conda packages
    let binding = lock_file
        .environment(environment_name.as_str())
        .and_then(|env| env.conda_repodata_records(platform).unwrap())
        .unwrap();
    let locked_pypi_data = binding
        .iter()
        .find(|record| is_python_record(&record.package_record))
        .unwrap();

    // Check if all the wheel tags match the system virtual packages
    let wheel_tags = all_wheel_tags(lock_file, platform, environment_name);
    let vpkgs = VirtualPackage::detect(&VirtualPackageOverrides::from_env())?;

    let system_tags =
        get_tags_from_machine(&vpkgs, platform, &locked_pypi_data.package_record).unwrap();

    Ok(true)
}

mod test {
    use super::*;
    use rattler_virtual_packages::Override;
    use std::path::Path;

    #[test]
    fn test_get_minimal_virtual_packages() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lockfile_path = root_dir.join("tests/data/lockfiles/cuda_virtual_dependency.lock");
        let lockfile = LockFile::from_path(&lockfile_path).unwrap();
        let platform = Platform::Linux64;

        let virtual_matchspecs = get_required_virtual_packages_from_conda_records(
            &lockfile,
            platform,
            &EnvironmentName::default(),
        );

        assert!(virtual_matchspecs
            .iter()
            .contains(&MatchSpec::from_str("__cuda >=12", Lenient).unwrap()));
    }

    #[test]
    fn test_validate_virtual_packages() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lockfile_path = root_dir.join("tests/data/lockfiles/cuda_virtual_dependency.lock");
        let lockfile = LockFile::from_path(&lockfile_path).unwrap();
        let platform = Platform::Linux64;

        // Override the virtual package to a version that is not available on the system
        let overrides = VirtualPackageOverrides {
            cuda: Some(Override::String("12.0".to_string())),
            ..VirtualPackageOverrides::default()
        };

        let result = validate_virtual_packages(
            &lockfile,
            platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        assert!(result.is_ok());

        // Override the virtual package to a version that is not available on the system
        let overrides = VirtualPackageOverrides {
            cuda: Some(Override::String("11.0".to_string())),
            ..VirtualPackageOverrides::default()
        };

        let result = validate_virtual_packages(
            &lockfile,
            platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        assert!(result.is_err());
    }
}
