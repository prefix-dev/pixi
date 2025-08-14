use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::Diagnostic;
use pixi_manifest::EnvironmentName;
use pypi_modifiers::pypi_tags::{PyPITagError, get_tags_from_machine, is_python_record};
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

/// Define accepted virtual packages as a constant set
/// These packages will be checked against the system virtual packages
const ACCEPTED_VIRTUAL_PACKAGES: &[&str] = &["__glibc", "__cuda", "__osx", "__win", "__linux"];

#[derive(Debug, Error, Diagnostic)]
#[error("{msg}")]
pub struct VirtualPackageNotFoundError {
    msg: String,
    #[help]
    help: Option<String>,
}

impl VirtualPackageNotFoundError {
    pub fn new(
        required_package: &MatchSpec,
        system_virtual_packages: &Vec<&GenericVirtualPackage>,
    ) -> Self {
        let override_var = if required_package
            .name
            .as_ref()
            .is_some_and(|name| name.as_normalized() == "__glibc")
        {
            // TODO: would be awesome to set the version based on the required version.
            // 2.17 is used as it's a good default
            Some("`CONDA_OVERRIDE_GLIBC=2.17`")
        } else if required_package
            .name
            .as_ref()
            .is_some_and(|name| name.as_normalized() == "__cuda")
        {
            Some("`CONDA_OVERRIDE_CUDA=12.0`")
        } else if required_package
            .name
            .as_ref()
            .is_some_and(|name| name.as_normalized() == "__osx")
        {
            Some("`CONDA_OVERRIDE_OSX=10.15`")
        } else {
            None
        };

        let help = override_var.map(|override_var| {
            format!(
            " You can mock the virtual package by overriding the environment variable, e.g.: '{}'",
            override_var
        )
        });

        let msg = format!(
            "Virtual package '{}' does not match any of the available virtual packages on your machine: [{}]",
            required_package,
            system_virtual_packages
                .iter()
                .map(|vpkg| vpkg.to_string())
                .join(", "),
        );
        VirtualPackageNotFoundError { msg, help }
    }
}

#[derive(Debug, Error, Diagnostic)]
#[error("Failed to validate that machine meets the requirements of the environment")]
pub enum MachineValidationError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    VirtualPackageNotFound(#[from] VirtualPackageNotFoundError),

    #[error("Couldn't get the virtual packages from the system")]
    VirtualPackageDetectionError(#[from] DetectVirtualPackageError),

    #[error(transparent)]
    RepodataConversionError(#[from] ConversionError),

    #[error("Couldn't parse dependencies")]
    DependencyParsingError(#[from] ParseMatchSpecError),

    #[error("Can't find environment: {0}")]
    EnvironmentNotFound(String),

    #[error(transparent)]
    #[diagnostic(transparent)]
    PyPITagError(#[from] PyPITagError),

    #[error("Wheel: {0} doesn't match this systems virtual capabilities for tags: {1}")]
    WheelTagsMismatch(String, String),

    #[error("No Python record found in the lockfile for platform: {0}.")]
    #[diagnostic(
        help = "Please make sure that 'python' is added in conda dependencies. Otherwise , please report this issue to the developers."
    )]
    NoPythonRecordFound(Platform),
}

/// Get the required virtual packages for the given environment based on the given lock file.
pub(crate) fn get_required_virtual_packages_from_conda_records(
    conda_records: &[&PackageRecord],
) -> Result<Vec<MatchSpec>, MachineValidationError> {
    // Collect all dependencies from the package records.
    let virtual_dependencies = conda_records
        .iter()
        .flat_map(|record| record.depends.iter().filter(|dep| dep.starts_with("__")))
        .collect_vec();

    // Convert the virtual dependencies into `MatchSpec`s.
    virtual_dependencies
        .iter()
        // Lenient parsing is used here because the dependencies to avoid issues with the parsing of the dependencies.
        // As the user can't do anything about the dependencies, we don't want to fail the whole process because of a parsing error.
        .map(|dep| MatchSpec::from_str(dep.as_str(), Lenient))
        .dedup()
        .collect::<Result<Vec<MatchSpec>, _>>()
        .map_err(MachineValidationError::DependencyParsingError)
}

/// Get the wheel filenames from the lockfile pypi package data
fn get_wheels_from_pypi_package_data(pypi_packages: Vec<PypiPackageData>) -> Vec<WheelFilename> {
    pypi_packages
        .into_iter()
        .map(|package| package.location.clone())
        .flat_map(|location| {
            if let Some(file_name) = location.file_name() {
                WheelFilename::from_str(file_name).ok()
            } else {
                tracing::debug!("No file name found for location: {:?}", location);
                None
            }
        })
        .collect_vec()
}

/// Validate that current machine has all the required virtual packages for the given environment
pub(crate) fn validate_system_meets_environment_requirements(
    lock_file: &LockFile,
    platform: Platform,
    environment_name: &EnvironmentName,
    virtual_package_overrides: Option<VirtualPackageOverrides>,
) -> Result<bool, MachineValidationError> {
    // Early out if there are no packages in the lockfile
    if lock_file.is_empty() {
        tracing::debug!("No packages in the lockfile, skipping virtual package validation");
        return Ok(true);
    }

    // Get the environment from the lock file
    let environment = lock_file.environment(environment_name.as_str()).ok_or(
        MachineValidationError::EnvironmentNotFound(environment_name.as_str().to_string()),
    )?;

    // Retrieve the conda package records for the specified platform.
    let Some(conda_data) = environment
        .conda_repodata_records(platform)
        .map_err(MachineValidationError::RepodataConversionError)?
    else {
        // Early out if there are no conda records, as we don't need to check for virtual packages
        return Ok(true);
    };

    let conda_records: Vec<&PackageRecord> = conda_data
        .iter()
        .map(|record| &record.package_record)
        .collect();

    // Get the virtual packages required by the conda records
    let required_virtual_packages =
        get_required_virtual_packages_from_conda_records(&conda_records)?;

    tracing::debug!(
        "Required virtual packages of environment '{}': {}",
        environment_name.fancy_display(),
        required_virtual_packages
            .iter()
            .map(|spec| spec.to_string())
            .join(", "),
    );

    // Default to the environment variable overrides, but allow for an override for testing
    let virtual_package_overrides =
        virtual_package_overrides.unwrap_or(VirtualPackageOverrides::from_env());

    // Get the virtual packages available on the system
    let system_virtual_packages = VirtualPackage::detect(&virtual_package_overrides)?;
    let generic_system_virtual_packages = system_virtual_packages
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();

    tracing::debug!(
        "Generic system virtual packages for env: '{}' : [{}]",
        environment_name.fancy_display(),
        generic_system_virtual_packages
            .iter()
            .map(|(name, vpkg)| format!("{}: {}", name.as_normalized(), vpkg))
            .join(", ")
    );

    // Check if all the required virtual conda packages match the system virtual packages
    for required in required_virtual_packages {
        // Check if the package name is in our accepted list
        let is_accepted = required
            .name
            .as_ref()
            .iter()
            .any(|name| ACCEPTED_VIRTUAL_PACKAGES.contains(&name.as_normalized()));

        // Skip if not in accepted packages
        if !is_accepted {
            tracing::debug!(
                "Skipping virtual package: {} as it's not in the accepted packages",
                required
            );
            continue;
        }

        let name = if let Some(name) = required.name.as_ref() {
            name
        } else {
            continue;
        };

        if let Some(local_vpkg) = generic_system_virtual_packages.get(name) {
            if !required.matches(local_vpkg) {
                return Err(VirtualPackageNotFoundError::new(
                    &required,
                    &generic_system_virtual_packages.values().collect(),
                )
                .into());
            }
            tracing::debug!("Required virtual package: {} matches the system", required);
        } else {
            return Err(VirtualPackageNotFoundError::new(
                &required,
                &generic_system_virtual_packages.values().collect(),
            )
            .into());
        }
    }

    // Check if the wheel tags match the system virtual packages if there are any
    if environment.has_pypi_packages(platform) {
        if let Some(pypi_packages) = environment.pypi_packages(platform) {
            // Get python record from conda packages
            let python_record = conda_records
                .iter()
                .find(|record| is_python_record(record))
                .ok_or(MachineValidationError::NoPythonRecordFound(platform))?;

            // Check if all the wheel tags match the system virtual packages
            let pypi_packages = pypi_packages
                .map(|(pkg_data, _)| pkg_data.clone())
                .collect_vec();

            let wheels = get_wheels_from_pypi_package_data(pypi_packages);

            let uv_system_tags =
                get_tags_from_machine(&system_virtual_packages, platform, python_record)?;

            // Check if all the wheel tags match the system virtual packages
            for wheel in wheels {
                if !wheel.is_compatible(&uv_system_tags) {
                    return Err(MachineValidationError::WheelTagsMismatch(
                        wheel.to_string(),
                        uv_system_tags.to_string(),
                    ));
                }
                tracing::debug!("Wheel: {} matches the system", wheel);
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod test {
    use super::*;
    use insta::assert_snapshot;
    use pixi_test_utils::format_diagnostic;
    use rattler_conda_types::ParseStrictness;
    use rattler_virtual_packages::Override;
    use std::path::Path;

    #[test]
    fn test_get_minimal_virtual_packages() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lockfile_path =
            root_dir.join("../../tests/data/lockfiles/cuda_virtual_dependency.lock");
        let lockfile = LockFile::from_path(&lockfile_path).unwrap();
        let platform = Platform::Linux64;
        let env = lockfile.default_environment().unwrap();
        let conda_data = env
            .conda_repodata_records(platform)
            .map_err(MachineValidationError::RepodataConversionError)
            .unwrap()
            .unwrap();

        let conda_records: Vec<&PackageRecord> = conda_data
            .iter()
            .map(|binding| &binding.package_record)
            .collect();

        let virtual_matchspecs =
            get_required_virtual_packages_from_conda_records(&conda_records).unwrap();

        assert!(
            virtual_matchspecs
                .iter()
                .contains(&MatchSpec::from_str("__cuda >=12", Lenient).unwrap())
        );
    }

    #[test]
    fn test_validate_virtual_packages() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lockfile_path =
            root_dir.join("../../tests/data/lockfiles/cuda_virtual_dependency.lock");
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
        let lockfile_path = root_dir.join("../../tests/data/lockfiles/pypi-numpy.lock");
        let lockfile = LockFile::from_path(&lockfile_path).unwrap();
        let platform = Platform::current();

        let overrides = VirtualPackageOverrides {
            // To high version for the wheel, which is fine as we assume backwards compatibility
            osx: Some(Override::String("15.1".to_string())),
            libc: Some(Override::String("2.9999".to_string())),
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
            osx: Some(Override::String("13.0".to_string())),
            libc: Some(Override::String("2.10".to_string())),
            ..VirtualPackageOverrides::default()
        };

        let result = validate_system_meets_environment_requirements(
            &lockfile,
            platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        if Platform::current().is_unix() {
            assert!(
                matches!(result, Err(MachineValidationError::WheelTagsMismatch(_, _))),
                "{:?}",
                result
            );
        } else {
            // It's hard to make the wheels fail on windows
            assert!(result.is_ok(), "{:?}", result);
        }
    }

    #[test]
    fn test_virtual_package_not_found_error() {
        // Create a test MatchSpec for glibc
        let spec = MatchSpec::from_str("__glibc >= 2.28", ParseStrictness::Strict).unwrap();

        // Define some available virtual packages
        let libc = GenericVirtualPackage {
            name: "__glibc".parse().unwrap(),
            version: "2.17".parse().unwrap(),
            build_string: "".to_string(),
        };
        let cuda = GenericVirtualPackage {
            name: "__cuda".parse().unwrap(),
            version: "11.8".parse().unwrap(),
            build_string: "".to_string(),
        };
        let osx = GenericVirtualPackage {
            name: "__osx".parse().unwrap(),
            version: "10.14".parse().unwrap(),
            build_string: "".to_string(),
        };
        let system_virtual_packages = vec![&libc, &cuda, &osx];

        let error1 = VirtualPackageNotFoundError::new(&spec, &system_virtual_packages);

        // Create a test MatchSpec for win which doesn't have an override
        let spec = MatchSpec::from_str("__win >= 1.2.3", ParseStrictness::Strict).unwrap();
        let error2 = VirtualPackageNotFoundError::new(&spec, &system_virtual_packages);

        assert_snapshot!(format!(
            "With override:\n{}\nWithout override:\n{}",
            format_diagnostic(&error1),
            format_diagnostic(&error2)
        ));
    }
    #[test]
    fn test_virtual_package_not_found_error_with_overrides() {
        // Check all overrides
        let overrides = vec![
            ("__glibc >= 2.17", "`CONDA_OVERRIDE_GLIBC=2.17`"),
            ("__cuda >= 12.0", "`CONDA_OVERRIDE_CUDA=12.0`"),
            ("__osx >= 10.15", "`CONDA_OVERRIDE_OSX=10.15`"),
        ];

        let system_virtual_packages = vec![];

        for (spec, msg) in overrides {
            let error = VirtualPackageNotFoundError::new(
                &MatchSpec::from_str(spec, ParseStrictness::Strict).unwrap(),
                &system_virtual_packages,
            );
            assert!(error.help.unwrap().contains(msg));
        }
    }

    #[test]
    fn test_archspec_skip() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lockfile_path = root_dir.join("../../tests/data/lockfiles/archspec.lock");
        let lockfile = LockFile::from_path(&lockfile_path).unwrap();
        let platform = Platform::Linux64;

        let overrides = VirtualPackageOverrides {
            libc: Some(Override::String("2.17".to_string())),
            ..VirtualPackageOverrides::default()
        };

        // validate that the archspec is skipped
        validate_system_meets_environment_requirements(
            &lockfile,
            platform,
            &EnvironmentName::default(),
            Some(overrides),
        )
        .unwrap();
    }

    #[test]
    fn test_ignored_virtual_packages() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lockfile_path =
            root_dir.join("../../tests/data/lockfiles/ignored_virtual_packages.lock");
        let lockfile = LockFile::from_path(&lockfile_path).unwrap();
        let platform = Platform::Linux64;

        let overrides = VirtualPackageOverrides {
            libc: Some(Override::String("2.17".to_string())),
            cuda: Some(Override::String("11.0".to_string())),
            ..VirtualPackageOverrides::default()
        };

        // validate that the ignored virtual packages are skipped
        validate_system_meets_environment_requirements(
            &lockfile,
            platform,
            &EnvironmentName::default(),
            Some(overrides),
        )
        .unwrap();
    }
}
