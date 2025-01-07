use itertools::Itertools;
use miette::Diagnostic;
use pixi_manifest::EnvironmentName;
use rattler_conda_types::ParseStrictness::Lenient;
use rattler_conda_types::Platform;
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, Matches};
use rattler_lock::LockFile;
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use std::collections::HashMap;
use thiserror::Error;

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
}

/// Get the required virtual packages for the given environment based on the given lock file.
pub(crate) fn get_required_virtual_packages(
    lock_file: &LockFile,
    platform: &Platform,
    environment_name: &EnvironmentName,
) -> Vec<MatchSpec> {
    // Get the locked conda packages for the given platform and environment
    let locked_records = lock_file
        .environment(environment_name.as_str())
        .and_then(|env| env.conda_repodata_records(*platform).unwrap())
        .unwrap();

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

/// Validate that current machine has all the required virtual packages for the given environment
pub(crate) fn validate_virtual_packages(
    lock_file: &LockFile,
    platform: &Platform,
    environment_name: &EnvironmentName,
    virtual_package_overrides: Option<VirtualPackageOverrides>,
) -> Result<bool, VirtualPackageError> {
    // Default to the environment variable overrides, but allow for an override for testing
    let virtual_package_overrides =
        virtual_package_overrides.unwrap_or(VirtualPackageOverrides::from_env());

    let required_virtual_packages =
        get_required_virtual_packages(lock_file, platform, environment_name);

    // Get the virtual packages available on the system
    let system_virtual_packages = VirtualPackage::detect(&virtual_package_overrides)
        .unwrap()
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();

    // Check if all the required virtual packages match the system virtual packages
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

        let virtual_matchspecs =
            get_required_virtual_packages(&lockfile, &platform, &EnvironmentName::default());

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
            &platform,
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
            &platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        assert!(result.is_err());
    }
}
