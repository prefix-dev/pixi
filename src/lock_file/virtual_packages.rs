use itertools::Itertools;
use miette::Diagnostic;
use pixi_manifest::EnvironmentName;
use pypi_modifiers::pypi_tags::{get_tags_from_machine, is_python_record};
use rattler_conda_types::ParseStrictness::Lenient;
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, Matches};
use rattler_conda_types::{ParseMatchSpecError, Platform};
use rattler_lock::{ConversionError, LockFile, PypiPackageData};
use rattler_virtual_packages::{
    DetectVirtualPackageError, VirtualPackage, VirtualPackageOverrides,
};
use std::collections::HashMap;
use std::str::FromStr;
use thiserror::Error;
use uv_distribution_filename::WheelFilename;

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

    #[error(transparent)]
    RepodataConversionError(#[from] ConversionError),

    #[error("No Conda records found in the lockfile for platform: {0}")]
    NoCondaRecordsFound(Platform),

    #[error("No PyPI packages found in the lockfile for platform: {0}")]
    NoPypiPackagesFound(Platform),

    #[error("Couldn't parse dependencies")]
    DependencyParsingError(#[from] ParseMatchSpecError),

    #[error("Can't find environment: {0}")]
    EnvironmentNotFound(String),

    #[error(transparent)]
    WheelTagsError(#[from] WheelTagsErrors),

    #[error("Wheel: {0} doesn't match this systems virtual capabilities")]
    WheelTagsMismatch(String),

    #[error("No Python record found in the lockfile for platform: {0}. This is not your fault, but a bug, please report it.")]
    NoPythonRecordFound(Platform),
}

/// Get the required virtual packages for the given environment based on the given lock file.
pub(crate) fn get_required_virtual_packages_from_conda_records(
    lock_file: &LockFile,
    platform: Platform,
    environment_name: &EnvironmentName,
) -> Result<Vec<MatchSpec>, VirtualPackageError> {
    // Get the locked conda packages for the given platform and environment
    let locked_env = lock_file.environment(environment_name.as_str()).unwrap();
    let locked_records = locked_env
        .conda_repodata_records(platform)
        .map_err(VirtualPackageError::RepodataConversionError)?
        .ok_or(VirtualPackageError::NoCondaRecordsFound(platform))?;

    let dependencies = locked_records
        .into_iter()
        .flat_map(|record| record.package_record.depends)
        .collect_vec();

    let match_specs = dependencies
        .iter()
        .map(|dep| MatchSpec::from_str(dep.as_str(), Lenient))
        .collect::<Result<Vec<MatchSpec>, _>>()
        .map_err(VirtualPackageError::DependencyParsingError)?;

    Ok(match_specs
        .into_iter()
        // Filter out the non-virtual dependencies
        .filter(|dep| dep.is_virtual())
        .collect_vec())
}

/// Wheel tags errors
#[derive(Debug, Error, Diagnostic)]
pub enum WheelTagsErrors {
    #[error("No PyPI packages found for platform: {0}")]
    NoPypiPackagesFound(Platform),
}

fn get_wheels_from_lockfile(
    pypi_packages: Vec<PypiPackageData>,
) -> Result<Vec<WheelFilename>, WheelTagsErrors> {
    Ok(pypi_packages
        .into_iter()
        .map(|package| package.location.clone())
        .flat_map(|location| {
            if let Some(file_name) = location.file_name() {
                // TODO: Handle errors
                WheelFilename::from_str(file_name).ok()
            } else {
                None
            }
        })
        .collect_vec())
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

    let environment = lock_file.environment(environment_name.as_str()).ok_or(
        VirtualPackageError::EnvironmentNotFound(environment_name.as_str().to_string()),
    )?;

    let required_virtual_packages =
        get_required_virtual_packages_from_conda_records(lock_file, platform, environment_name)?;

    // Get the virtual packages available on the system
    let system_virtual_packages = VirtualPackage::detect(&virtual_package_overrides)?;
    let generic_virtual_packages = system_virtual_packages
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();

    // Check if all the required virtual conda packages match the system virtual packages
    for required in required_virtual_packages {
        if let Some(local_vpkg) = generic_virtual_packages.get(
            &required
                .name
                .clone()
                .expect("Virtual package name not found"),
        ) {
            // TODO: Handle errors
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

    // Check if the wheel tags match the system virtual packages if there are any
    if environment.pypi_packages(platform).is_some()
        && environment
            .pypi_packages(platform)
            // TODO: Handle errors
            .unwrap()
            .next()
            .is_some()
    {
        // Get python record from conda packages
        let binding = environment
            .conda_repodata_records(platform)?
            .ok_or(VirtualPackageError::NoCondaRecordsFound(platform))?;
        let python_record = binding
            .iter()
            .find(|record| is_python_record(&record.package_record))
            .ok_or(VirtualPackageError::NoPythonRecordFound(platform))?;

        // Check if all the wheel tags match the system virtual packages
        let pypi_packages = environment
            .pypi_packages(platform)
            .ok_or(VirtualPackageError::NoPypiPackagesFound(platform))?
            .map(|(pkg_data, _env_data)| pkg_data.clone())
            .collect_vec();

        let wheels = get_wheels_from_lockfile(pypi_packages)?;

        let system_tags = get_tags_from_machine(
            &system_virtual_packages,
            platform,
            &python_record.package_record,
        )
        // TODO: Handle errors
        .unwrap();

        // Check if all the wheel tags match the system virtual packages
        for wheel in wheels {
            if wheel.is_compatible(&system_tags) {
                // TODO: Handle errors
                continue;
            } else {
                return Err(VirtualPackageError::WheelTagsMismatch(wheel.to_string()));
            }
        }
    }

    Ok(true)
}

#[cfg(test)]
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
        )
        .unwrap();

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
        assert!(result.is_ok(), "{:?}", result);

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

    #[test]
    fn test_validate_wheel_tags() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lockfile_path = root_dir.join("tests/data/lockfiles/pypi-numpy.lock");
        let lockfile = LockFile::from_path(&lockfile_path).unwrap();
        let platform = Platform::OsxArm64;

        let overrides = VirtualPackageOverrides {
            osx: Some(Override::String("15.1".to_string())),
            ..VirtualPackageOverrides::default()
        };

        let result = validate_virtual_packages(
            &lockfile,
            platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        assert!(result.is_ok(), "{:?}", result);

        let overrides = VirtualPackageOverrides {
            // To low version for the wheel
            osx: Some(Override::String("14.0".to_string())),
            ..VirtualPackageOverrides::default()
        };

        let result = validate_virtual_packages(
            &lockfile,
            platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        assert!(
            matches!(result, Err(VirtualPackageError::WheelTagsMismatch(_))),
            "{:?}",
            result
        );
    }
}
