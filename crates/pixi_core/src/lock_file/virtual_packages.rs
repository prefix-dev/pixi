use crate::workspace::errors::conda_override_hint;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::Diagnostic;
use pixi_manifest::{EnvironmentName, PixiPlatform, PixiPlatformName};
use pypi_modifiers::pypi_tags::{PyPITagError, get_tags_from_machine, is_python_record};
use rattler_conda_types::ParseMatchSpecError;
use rattler_conda_types::ParseStrictness::Lenient;
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, Matches, PackageName, Platform, Version, VersionSpec,
};
use rattler_lock::{CondaPackageData, ConversionError, LockFile, PypiPackageData};
use rattler_virtual_packages::{
    DetectVirtualPackageError, VirtualPackage, VirtualPackageOverrides,
};
use std::collections::HashMap;
use std::str::FromStr;
use thiserror::Error;
use uv_distribution_filename::WheelFilename;

/// Define accepted virtual packages as a constant set
/// These packages will be checked against the system virtual packages
const ACCEPTED_VIRTUAL_PACKAGES: &[&str] = &[
    "__glibc", "__musl", "__eglibc", "__cuda", "__osx", "__win", "__linux",
];

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
        let required_version = required_package.version.as_ref().and_then(spec_version);
        let help = required_package
            .name
            .as_exact()
            .and_then(|name| conda_override_hint(name.as_normalized(), required_version))
            .map(|hint| {
                format!(
                    " You can mock the virtual package by overriding the environment variable, e.g.: '`{hint}`'"
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

    #[error("No Python record found in the lock file for platform: {0}.")]
    #[diagnostic(
        help = "Please make sure that 'python' is added in conda dependencies. Otherwise , please report this issue to the developers."
    )]
    NoPythonRecordFound(PixiPlatformName),
}

/// Get the required virtual packages from dependency strings.
pub(crate) fn get_required_virtual_packages_from_depends(
    depends: &[&str],
) -> Result<Vec<MatchSpec>, MachineValidationError> {
    depends
        .iter()
        .filter(|dep| dep.starts_with("__"))
        .map(|dep| MatchSpec::from_str(dep, Lenient))
        .dedup()
        .collect::<Result<Vec<MatchSpec>, _>>()
        .map_err(MachineValidationError::DependencyParsingError)
}

/// The single version a virtual-package match spec carries. Virtual-package
/// dependencies are always pinned to one exact version (`__cuda 12` /
/// `__cuda >=12` both mean version `12`), so the operator is irrelevant -- we
/// just read the version. Specs with no version (a bare `__cuda`) yield `None`.
fn spec_version(spec: &VersionSpec) -> Option<&Version> {
    match spec {
        VersionSpec::Range(_, version) | VersionSpec::Exact(_, version) => Some(version),
        _ => None,
    }
}

/// Compute the minimal-required platform for each subdir the environment was
/// resolved for: the subdir plus exactly the virtual packages that some resolved
/// dependency requires, each at the highest version seen across all packages.
///
/// The result is keyed by subdir; `declared_platforms` that share a subdir are
/// unioned. Only `depends` is considered, mirroring
/// [`validate_system_meets_environment_requirements`]. A subdir whose lock-file
/// entry has no conda packages is omitted (the caller falls back to the declared
/// platform); a subdir with packages but no virtual-package requirements yields a
/// platform with an empty declared set.
pub(crate) fn compute_minimal_required_platforms(
    lock_file: &LockFile,
    environment_name: &EnvironmentName,
    declared_platforms: &[&PixiPlatform],
) -> HashMap<Platform, PixiPlatform> {
    let Some(environment) = lock_file.environment(environment_name.as_str()) else {
        return HashMap::new();
    };

    // subdir -> all `depends` strings of its resolved conda packages, unioned
    // across the declared platforms that share a subdir.
    let mut depends_by_subdir: HashMap<Platform, Vec<String>> = HashMap::new();

    for platform in declared_platforms {
        let lock_platform = super::resolve_lock_platform_for(environment.lock_file(), platform);
        let Some(conda_packages) = lock_platform.and_then(|p| environment.conda_packages(p)) else {
            continue;
        };
        let entry = depends_by_subdir.entry(platform.subdir()).or_default();
        entry.extend(
            conda_packages
                .flat_map(|data| data.depends())
                .map(ToString::to_string),
        );
    }

    depends_by_subdir
        .into_iter()
        .map(|(subdir, depends)| {
            let depends: Vec<&str> = depends.iter().map(String::as_str).collect_vec();
            (
                subdir,
                PixiPlatform::from_required_virtual_packages(
                    subdir,
                    minimal_required_virtual_packages(&depends),
                ),
            )
        })
        .collect()
}

/// The virtual packages that some dependency in `depends` requires: each
/// accepted virtual package at the highest version seen across all `depends`,
/// with a version-less requirement (bare `__cuda`) pinned to version 0 so it
/// survives but loses to any versioned one. The result is sorted by name.
///
/// This is the per-subdir core of `compute_minimal_required_platforms`,
/// shared with `pixi global` which derives the same minimum from an installed
/// environment's records rather than a lock file.
pub fn minimal_required_virtual_packages(depends: &[&str]) -> Vec<GenericVirtualPackage> {
    let Ok(specs) = get_required_virtual_packages_from_depends(depends) else {
        return Vec::new();
    };

    let mut aggregated: HashMap<PackageName, GenericVirtualPackage> = HashMap::new();
    for spec in specs {
        let Some(name) = spec.name.as_exact() else {
            continue;
        };
        let version = spec
            .version
            .as_ref()
            .and_then(spec_version)
            .cloned()
            .unwrap_or_else(|| Version::major(0));
        aggregated
            .entry(name.clone())
            .and_modify(|existing| {
                if version > existing.version {
                    existing.version = version.clone();
                }
            })
            .or_insert_with(|| GenericVirtualPackage {
                name: name.clone(),
                version,
                build_string: String::new(),
            });
    }

    let mut vps: Vec<GenericVirtualPackage> = aggregated.into_values().collect();
    vps.sort_by(|a, b| a.name.as_normalized().cmp(b.name.as_normalized()));
    vps
}

/// Get the wheel filenames from the lock file pypi package data
fn get_wheels_from_pypi_package_data(pypi_packages: Vec<PypiPackageData>) -> Vec<WheelFilename> {
    pypi_packages
        .into_iter()
        .map(|package| package.location().clone())
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
    platform: &PixiPlatform,
    environment_name: &EnvironmentName,
    virtual_package_overrides: Option<VirtualPackageOverrides>,
) -> Result<bool, MachineValidationError> {
    // Early out if there are no packages in the lock file
    if lock_file.is_empty() {
        tracing::debug!("No packages in the lock file, skipping virtual package validation");
        return Ok(true);
    }

    // Get the environment from the lock file
    let environment = lock_file.environment(environment_name.as_str()).ok_or(
        MachineValidationError::EnvironmentNotFound(environment_name.as_str().to_string()),
    )?;

    // Retrieve all conda packages for the specified platform (both binary and source).
    let lock_platform = super::resolve_lock_platform_for(environment.lock_file(), platform);
    let Some(conda_packages) = lock_platform.and_then(|p| environment.conda_packages(p)) else {
        // Early out if there are no packages, as we don't need to check for virtual packages
        return Ok(true);
    };

    // Collect conda packages (both binary and source) into a vector of CondaPackageData
    let conda_packages: Vec<&CondaPackageData> = conda_packages.collect_vec();

    if conda_packages.is_empty() {
        // Early out if there are no conda records, as we don't need to check for virtual packages
        return Ok(true);
    }

    // Get depends from all packages (binary and source, including partial)
    let all_depends: Vec<&str> = conda_packages
        .iter()
        .flat_map(|data| data.depends())
        .map(|s| s.as_str())
        .collect_vec();

    // Get the virtual packages required by the conda records
    let required_virtual_packages = get_required_virtual_packages_from_depends(&all_depends)?;

    // Find the python package record (needed for wheel tag validation below).
    // This works for binary and full source packages; partial source records
    // don't have a PackageRecord and are skipped.
    let python_record = conda_packages
        .iter()
        .filter_map(|data| data.record())
        .find(|record| is_python_record(record));

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
            .as_exact()
            .map(|n| ACCEPTED_VIRTUAL_PACKAGES.contains(&n.as_normalized()))
            .unwrap_or(false);

        // Skip if not in accepted packages
        if !is_accepted {
            tracing::debug!(
                "Skipping virtual package: {} as it's not in the accepted packages",
                required
            );
            continue;
        }

        let name = if let Some(name_exact) = required.name.as_exact() {
            name_exact
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
    if lock_platform.is_some_and(|p| environment.has_pypi_packages(p))
        && let Some(pypi_packages) = lock_platform.and_then(|p| environment.pypi_packages(p))
    {
        let python_record = python_record
            .ok_or_else(|| MachineValidationError::NoPythonRecordFound(platform.name().clone()))?;

        // Check if all the wheel tags match the system virtual packages
        let pypi_packages = pypi_packages.cloned().collect_vec();

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
    Ok(true)
}

#[cfg(test)]
mod test {
    use super::*;
    use insta::assert_snapshot;
    use pixi_test_utils::format_diagnostic;
    use rattler_conda_types::package::DistArchiveIdentifier;
    use rattler_conda_types::{PackageRecord, ParseStrictness, Platform};
    use rattler_lock::{CondaBinaryData, PlatformData, PlatformName, UrlOrPath};
    use rattler_virtual_packages::Override;
    use std::path::Path;
    use url::Url;

    #[test]
    fn test_get_minimal_virtual_packages() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lock_file_path =
            root_dir.join("../../tests/data/lock_files/cuda_virtual_dependency.lock");
        let lock_file = LockFile::from_path(&lock_file_path).unwrap();
        let platform = Platform::Linux64;
        let env = lock_file.default_environment().unwrap();
        let lock_platform = lock_file.platform(&platform.to_string()).unwrap();
        let conda_packages = env
            .conda_packages(lock_platform)
            .unwrap()
            .collect::<Vec<_>>();

        let all_depends: Vec<&str> = conda_packages
            .iter()
            .flat_map(|data| data.depends())
            .map(|s| s.as_str())
            .collect();

        let virtual_matchspecs = get_required_virtual_packages_from_depends(&all_depends).unwrap();

        assert!(
            virtual_matchspecs
                .iter()
                .contains(&MatchSpec::from_str("__cuda >=12", Lenient).unwrap())
        );
    }

    #[test]
    fn test_spec_version() {
        assert_eq!(
            spec_version(&VersionSpec::from_str(">=12", Lenient).unwrap()),
            Some(&Version::from_str("12").unwrap()),
        );
        assert_eq!(
            spec_version(&VersionSpec::from_str("==12.0", Lenient).unwrap()),
            Some(&Version::from_str("12.0").unwrap()),
        );
        assert_eq!(spec_version(&VersionSpec::Any), None);
    }

    #[test]
    fn test_compute_minimal_required_platforms() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lock_file_path =
            root_dir.join("../../tests/data/lock_files/cuda_virtual_dependency.lock");
        let lock_file = LockFile::from_path(&lock_file_path).unwrap();
        let declared = PixiPlatform::from_subdir(Platform::Linux64);

        let minimal = compute_minimal_required_platforms(
            &lock_file,
            &EnvironmentName::default(),
            &[&declared],
        );

        let platform = minimal
            .get(&Platform::Linux64)
            .expect("linux-64 minimal platform");
        assert_eq!(platform.subdir(), Platform::Linux64);

        // The resolved packages require `__cuda` at the highest version seen.
        let cuda = platform
            .declared_virtual_packages()
            .iter()
            .find(|vp| vp.name.as_normalized() == "__cuda")
            .expect("__cuda is required");
        assert_eq!(cuda.version, Version::from_str("12").unwrap());

        // Only depended-on VPs are present; subdir defaults are not padded in
        // (`__archspec` is a linux-64 default but never appears in `depends`).
        assert!(
            !platform
                .declared_virtual_packages()
                .iter()
                .any(|vp| vp.name.as_normalized() == "__archspec")
        );
    }

    /// A version-less virtual-package dependency (bare `__cuda`) still
    /// requires the package to be present. It used to be dropped from the
    /// minimal platform, making machines without the package look compatible
    /// while `validate_system_meets_environment_requirements` rejected them.
    #[test]
    fn test_compute_minimal_required_platforms_versionless_spec() {
        let lock_source = r#"version: 7
platforms:
- name: linux-64
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages:
      linux-64:
      - conda: https://conda.anaconda.org/conda-forge/linux-64/foo-1.0-h0.conda
packages:
- conda: https://conda.anaconda.org/conda-forge/linux-64/foo-1.0-h0.conda
  depends:
  - __cuda
"#;
        let lock_file = LockFile::from_str_with_base_directory(lock_source, None).unwrap();
        let declared = PixiPlatform::from_subdir(Platform::Linux64);

        let minimal = compute_minimal_required_platforms(
            &lock_file,
            &EnvironmentName::default(),
            &[&declared],
        );

        let platform = minimal
            .get(&Platform::Linux64)
            .expect("linux-64 minimal platform");
        let cuda = platform
            .declared_virtual_packages()
            .iter()
            .find(|vp| vp.name.as_normalized() == "__cuda")
            .expect("bare __cuda must survive into the minimal platform");
        assert_eq!(cuda.version, Version::major(0));
    }

    #[test]
    fn test_validate_virtual_packages() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lock_file_path =
            root_dir.join("../../tests/data/lock_files/cuda_virtual_dependency.lock");
        let lock_file = LockFile::from_path(&lock_file_path).unwrap();
        let platform = pixi_manifest::PixiPlatform::from_subdir(Platform::Linux64);

        // Override the virtual package to a version that is not available on the system
        let overrides = VirtualPackageOverrides {
            cuda: Some(Override::String("12.0".to_string())),
            ..VirtualPackageOverrides::default()
        };

        let result = validate_system_meets_environment_requirements(
            &lock_file,
            &platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        assert!(result.is_ok(), "{result:?}");

        // Override the virtual package to a version that is not available on the system
        let overrides = VirtualPackageOverrides {
            cuda: Some(Override::String("11.0".to_string())),
            ..VirtualPackageOverrides::default()
        };

        let result = validate_system_meets_environment_requirements(
            &lock_file,
            &platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        assert!(result.is_err());
    }

    /// Build a single-package linux-64 lock file whose lone conda package
    /// carries `depends`, used to drive the required-virtual-package check.
    fn lock_requiring(depends: &str) -> LockFile {
        let mut record = PackageRecord::new(
            PackageName::new_unchecked("needs-libc"),
            Version::from_str("1.0").unwrap(),
            "0".to_string(),
        );
        record.subdir = "linux-64".to_string();
        record.depends = vec![depends.to_string()];
        let package = CondaPackageData::Binary(Box::new(CondaBinaryData {
            package_record: record,
            location: UrlOrPath::Url(
                Url::parse("https://example.com/needs-libc-1.0-0.conda").unwrap(),
            )
            .into(),
            file_name: DistArchiveIdentifier::try_from_filename("needs-libc-1.0-0.conda").unwrap(),
            channel: None,
        }));
        let mut builder = LockFile::builder()
            .with_platforms(vec![PlatformData {
                name: PlatformName::try_from("linux-64").unwrap(),
                subdir: Platform::Linux64,
                virtual_packages: vec![],
            }])
            .unwrap();
        builder.set_channels("default", Vec::<rattler_lock::Channel>::new());
        builder.set_options("default", rattler_lock::SolveOptions::default());
        builder
            .add_conda_package("default", "linux-64", package)
            .unwrap();
        builder.finish()
    }

    /// `__musl`/`__eglibc` are verified at run-time like `__glibc`, not silently
    /// skipped. Forcing the libc slot to glibc means the host never reports
    /// `__musl`, so a musl-requiring environment must fail verification
    /// regardless of the test machine.
    #[test]
    fn musl_requirement_is_verified_not_skipped() {
        let lock_file = lock_requiring("__musl >=1.2");
        let platform = pixi_manifest::PixiPlatform::from_subdir(Platform::Linux64);
        let overrides = VirtualPackageOverrides {
            libc: Some(Override::String("2.28".to_string())),
            ..VirtualPackageOverrides::default()
        };

        let result = validate_system_meets_environment_requirements(
            &lock_file,
            &platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        assert!(
            matches!(
                result,
                Err(MachineValidationError::VirtualPackageNotFound(_))
            ),
            "{result:?}"
        );
    }

    #[test]
    fn test_validate_wheel_tags() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lock_file_path = root_dir.join("../../tests/data/lock_files/pypi-numpy.lock");
        let lock_file = LockFile::from_path(&lock_file_path).unwrap();
        let platform = pixi_manifest::PixiPlatform::from_subdir(Platform::current());

        let overrides = VirtualPackageOverrides {
            // To high version for the wheel, which is fine as we assume backwards compatibility
            osx: Some(Override::String("15.1".to_string())),
            libc: Some(Override::String("2.9999".to_string())),
            ..VirtualPackageOverrides::default()
        };

        let result = validate_system_meets_environment_requirements(
            &lock_file,
            &platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        assert!(result.is_ok(), "{result:?}");

        let overrides = VirtualPackageOverrides {
            // To low version for the wheel
            osx: Some(Override::String("13.0".to_string())),
            libc: Some(Override::String("2.10".to_string())),
            ..VirtualPackageOverrides::default()
        };

        let result = validate_system_meets_environment_requirements(
            &lock_file,
            &platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        if Platform::current().is_unix() {
            assert!(
                matches!(result, Err(MachineValidationError::WheelTagsMismatch(_, _))),
                "{result:?}"
            );
        } else {
            // It's hard to make the wheels fail on windows
            assert!(result.is_ok(), "{result:?}");
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

        // Create a test MatchSpec for unix which doesn't have an override
        let spec = MatchSpec::from_str("__unix >= 1.2.3", ParseStrictness::Strict).unwrap();
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
        let lock_file_path = root_dir.join("../../tests/data/lock_files/archspec.lock");
        let lock_file = LockFile::from_path(&lock_file_path).unwrap();
        let platform = pixi_manifest::PixiPlatform::from_subdir(Platform::Linux64);

        let overrides = VirtualPackageOverrides {
            libc: Some(Override::String("2.17".to_string())),
            ..VirtualPackageOverrides::default()
        };

        // validate that the archspec is skipped
        validate_system_meets_environment_requirements(
            &lock_file,
            &platform,
            &EnvironmentName::default(),
            Some(overrides),
        )
        .unwrap();
    }

    #[test]
    fn test_ignored_virtual_packages() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lock_file_path =
            root_dir.join("../../tests/data/lock_files/ignored_virtual_packages.lock");
        let lock_file = LockFile::from_path(&lock_file_path).unwrap();
        let platform = pixi_manifest::PixiPlatform::from_subdir(Platform::Linux64);

        let overrides = VirtualPackageOverrides {
            libc: Some(Override::String("2.17".to_string())),
            cuda: Some(Override::String("11.0".to_string())),
            ..VirtualPackageOverrides::default()
        };

        // validate that the ignored virtual packages are skipped
        validate_system_meets_environment_requirements(
            &lock_file,
            &platform,
            &EnvironmentName::default(),
            Some(overrides),
        )
        .unwrap();
    }
}
