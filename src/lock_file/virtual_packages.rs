use itertools::Itertools;
use miette::Diagnostic;
use pixi_manifest::EnvironmentName;
use pypi_modifiers::pypi_tags::{get_tags_from_machine, is_python_record, PyPITagError};
use rattler_conda_types::ParseStrictness::Lenient;
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, Matches, PackageRecord};
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

    #[error(transparent)]
    PyPITagError(#[from] PyPITagError),

    #[error("Wheel: {0} doesn't match this systems virtual capabilities")]
    WheelTagsMismatch(String),

    #[error("No Python record found in the lockfile for platform: {0}. This is not your fault, but a bug, please report it.")]
    NoPythonRecordFound(Platform),
}

/// Get the required virtual packages for the given environment based on the given lock file.
pub(crate) fn get_required_virtual_packages_from_conda_records(
    conda_records: &[&PackageRecord],
) -> Result<Vec<MatchSpec>, VirtualPackageError> {
    // Collect all dependencies from the package records.
    let virtual_dependencies = conda_records
        .iter()
        .flat_map(|record| record.depends.iter().filter(|dep| dep.starts_with("__")))
        .collect_vec();

    // Convert the virtual dependencies into `MatchSpec`s.
    virtual_dependencies
        .iter()
        // Lenient parsing is used here because the dependencies to avoid issues with the parsing of the dependencies.
        .map(|dep| MatchSpec::from_str(dep.as_str(), Lenient))
        .collect::<Result<Vec<MatchSpec>, _>>()
        .map_err(VirtualPackageError::DependencyParsingError)
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
pub(crate) fn validate_system_meets_environment_requirements(
    lock_file: &LockFile,
    platform: Platform,
    environment_name: &EnvironmentName,
    virtual_package_overrides: Option<VirtualPackageOverrides>,
) -> Result<bool, VirtualPackageError> {
    // Early out if there are no packages in the lockfile
    if lock_file.is_empty() {
        return Ok(true);
    }

    // Default to the environment variable overrides, but allow for an override for testing
    let virtual_package_overrides =
        virtual_package_overrides.unwrap_or(VirtualPackageOverrides::from_env());

    // Get the environment from the lock file
    let environment = lock_file.environment(environment_name.as_str()).ok_or(
        VirtualPackageError::EnvironmentNotFound(environment_name.as_str().to_string()),
    )?;

    // Retrieve the conda package records for the specified platform.
    let conda_data = environment
        .conda_repodata_records(platform)
        .map_err(VirtualPackageError::RepodataConversionError)?
        .ok_or(VirtualPackageError::NoCondaRecordsFound(platform))?;

    let conda_records: Vec<&PackageRecord> = conda_data
        .iter()
        .map(|binding| &binding.package_record)
        .collect();

    // Get the virtual packages required by the conda records
    let required_virtual_packages =
        get_required_virtual_packages_from_conda_records(&conda_records)?;

    // Get the virtual packages available on the system
    let system_virtual_packages = VirtualPackage::detect(&virtual_package_overrides)?;
    let generic_system_virtual_packages = system_virtual_packages
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();

    // Check if all the required virtual conda packages match the system virtual packages
    for required in required_virtual_packages {
        if let Some(local_vpkg) = generic_system_virtual_packages.get(
            &required
                .name
                .clone()
                .expect("Virtual packages should have a name"),
        ) {
            if !required.matches(local_vpkg) {
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
    if environment.has_pypi_packages(platform) {
        // Get python record from conda packages
        let python_record = conda_records
            .iter()
            .find(|record| is_python_record(record))
            .ok_or(VirtualPackageError::NoPythonRecordFound(platform))?;

        // Check if all the wheel tags match the system virtual packages
        let pypi_packages = environment
            .pypi_packages(platform)
            .expect("environment should have pypi packages")
            .map(|(pkg_data, _env_data)| pkg_data.clone())
            .collect_vec();

        let wheels = get_wheels_from_lockfile(pypi_packages)?;

        let system_tags = get_tags_from_machine(&system_virtual_packages, platform, python_record)?;

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
        let env = lockfile.default_environment().unwrap();
        let conda_data = env
            .conda_repodata_records(platform)
            .map_err(VirtualPackageError::RepodataConversionError)
            .unwrap()
            .ok_or(VirtualPackageError::NoCondaRecordsFound(platform))
            .unwrap();

        let conda_records: Vec<&PackageRecord> = conda_data
            .iter()
            .map(|binding| &binding.package_record)
            .collect();

        let virtual_matchspecs =
            get_required_virtual_packages_from_conda_records(&conda_records).unwrap();

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

        let result = validate_system_meets_environment_requirements(
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

        let result = validate_system_meets_environment_requirements(
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

        let result = validate_system_meets_environment_requirements(
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

        let result = validate_system_meets_environment_requirements(
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
