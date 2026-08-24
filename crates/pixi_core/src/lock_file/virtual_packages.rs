use crate::workspace::errors::spec_override_hint;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::Diagnostic;
use pixi_manifest::{EnvironmentName, PixiPlatform, PixiPlatformName};
use pypi_modifiers::pypi_tags::{PyPITagError, get_tags_from_machine, is_python_record};
use rattler_conda_types::ParseMatchSpecError;
use rattler_conda_types::ParseStrictness::Lenient;
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, Matches, Platform, Version, VersionSpec,
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
    /// `target` is the subdir the environment is being validated for.
    pub fn new(
        required_package: &MatchSpec,
        system_virtual_packages: &[GenericVirtualPackage],
        target: Platform,
    ) -> Self {
        let help = spec_override_hint(required_package, target).map(|hint| {
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

/// The requirements in `specs` that `system` does not satisfy, in order.
pub(crate) fn unmet_requirements(
    specs: &[MatchSpec],
    system: &[GenericVirtualPackage],
) -> Vec<MatchSpec> {
    let (checked, skipped): (Vec<&MatchSpec>, Vec<&MatchSpec>) = specs.iter().partition(|spec| {
        spec.name
            .as_exact()
            .is_some_and(|name| ACCEPTED_VIRTUAL_PACKAGES.contains(&name.as_normalized()))
    });
    if !skipped.is_empty() {
        tracing::debug!(
            "Not checking virtual-package requirements pixi cannot evaluate: {}",
            skipped.iter().map(ToString::to_string).join(", ")
        );
    }

    checked
        .into_iter()
        .filter(|spec| !system.iter().any(|provided| spec.matches(provided)))
        .cloned()
        .collect()
}

/// The version literal a virtual-package match spec mentions, ignoring its
/// operator. Specs with no version (a bare `__cuda`) yield `None`.
pub(crate) fn spec_version(spec: &VersionSpec) -> Option<&Version> {
    match spec {
        VersionSpec::Range(_, version) | VersionSpec::Exact(_, version) => Some(version),
        _ => None,
    }
}

/// Compute the virtual-package requirements for each subdir the environment was
/// resolved for: exactly the specs that some resolved dependency requires.
pub(crate) fn compute_required_virtual_package_specs(
    lock_file: &LockFile,
    environment_name: &EnvironmentName,
    declared_platforms: &[&PixiPlatform],
) -> HashMap<Platform, Vec<MatchSpec>> {
    let Some(environment) = lock_file.environment(environment_name.as_str()) else {
        return HashMap::new();
    };

    // subdir -> all `depends` strings of its resolved conda packages, across the
    // declared platforms that share a subdir.
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
            (subdir, required_virtual_package_specs(&depends))
        })
        .collect()
}

/// The virtual-package requirements some dependency in `depends` places on the
/// machine: the specs themselves, deduplicated and sorted so the recorded set is
/// stable.
///
/// The specs are kept verbatim rather than folded into one concrete virtual
/// package per name.
pub fn required_virtual_package_specs(depends: &[&str]) -> Vec<MatchSpec> {
    let Ok(specs) = get_required_virtual_packages_from_depends(depends) else {
        return Vec::new();
    };

    specs
        .into_iter()
        // A spec with a non-exact name matches nothing we can check.
        .filter(|spec| spec.name.as_exact().is_some())
        .map(|spec| (spec.to_string(), spec))
        .sorted_by(|(a, _), (b, _)| a.cmp(b))
        .dedup_by(|(a, _), (b, _)| a == b)
        .map(|(_, spec)| spec)
        .collect()
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
    let system_virtual_packages = VirtualPackage::detect(&virtual_package_overrides, None)?;
    let system_capabilities: Vec<GenericVirtualPackage> = system_virtual_packages
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .collect();

    tracing::debug!(
        "Generic system virtual packages for env: '{}' : [{}]",
        environment_name.fancy_display(),
        system_capabilities
            .iter()
            .map(ToString::to_string)
            .join(", ")
    );

    // Check if all the required virtual conda packages match the system virtual packages.
    if let Some(unmet) =
        unmet_requirements(&required_virtual_packages, &system_capabilities).first()
    {
        return Err(VirtualPackageNotFoundError::new(
            unmet,
            &system_capabilities,
            platform.subdir(),
        )
        .into());
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
    use rattler_conda_types::{PackageName, PackageRecord, ParseStrictness, Platform};
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
    fn test_compute_required_virtual_package_specs() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lock_file_path =
            root_dir.join("../../tests/data/lock_files/cuda_virtual_dependency.lock");
        let lock_file = LockFile::from_path(&lock_file_path).unwrap();
        let declared = PixiPlatform::from_subdir(Platform::Linux64);

        let required = compute_required_virtual_package_specs(
            &lock_file,
            &EnvironmentName::default(),
            &[&declared],
        );

        let specs = required
            .get(&Platform::Linux64)
            .expect("linux-64 requirements");
        // Only depended-on virtual packages are present; subdir defaults are
        // not padded in (`__archspec` is a linux-64 default but never appears
        // in `depends`).
        assert_eq!(
            specs.iter().map(ToString::to_string).collect_vec(),
            vec!["__cuda >=12"]
        );
    }

    #[test]
    fn test_required_specs_keep_a_versionless_spec_unconstrained() {
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

        let required = compute_required_virtual_package_specs(
            &lock_file,
            &EnvironmentName::default(),
            &[&declared],
        );

        let specs = required
            .get(&Platform::Linux64)
            .expect("linux-64 requirements");
        assert_eq!(
            specs.iter().map(ToString::to_string).collect_vec(),
            vec!["__cuda"]
        );
        assert!(specs[0].version.is_none());
    }

    #[test]
    fn test_required_specs_keep_every_distinct_constraint() {
        let depends = [
            "__glibc >=2.17",
            "__glibc >=2.28",
            "__archspec 1.* ^(x86_64_v3|skylake)$",
            // A duplicate of the first: recorded once.
            "__glibc >=2.17",
            // Not a virtual package: ignored entirely.
            "python >=3.12",
        ];
        let specs = required_virtual_package_specs(&depends);

        assert_eq!(
            specs.iter().map(ToString::to_string).collect_vec(),
            vec![
                "__archspec 1.*[build=\"^(x86_64_v3|skylake)$\"]",
                "__glibc >=2.17",
                "__glibc >=2.28",
            ]
        );
        // The build-string pattern survives as a pattern, not as a literal.
        assert!(matches!(
            specs[0].build,
            Some(rattler_conda_types::StringMatcher::Regex(_))
        ));
    }

    #[test]
    fn unmet_requirements_matches_the_spec_not_just_its_version() {
        let spec = |raw: &str| {
            MatchSpec::from_str(raw, rattler_conda_types::ParseStrictness::Lenient).unwrap()
        };
        let cuda = |v: &str| {
            vec![GenericVirtualPackage {
                name: rattler_conda_types::PackageName::try_from("__cuda").unwrap(),
                version: rattler_conda_types::Version::from_str(v).unwrap(),
                build_string: String::new(),
            }]
        };

        // A lower bound behaves as before: any newer machine satisfies it.
        assert!(unmet_requirements(&[spec("__cuda >=12")], &cuda("12.4")).is_empty());
        assert_eq!(
            unmet_requirements(&[spec("__cuda >=12")], &cuda("11")).len(),
            1
        );

        // An exact pin is honored as a pin. The old `>=`-only comparison called
        // this satisfied.
        assert_eq!(
            unmet_requirements(&[spec("__cuda ==12")], &cuda("12.4")).len(),
            1
        );

        // A bare requirement only asks for presence.
        assert!(unmet_requirements(&[spec("__cuda")], &cuda("11")).is_empty());
        assert_eq!(unmet_requirements(&[spec("__cuda")], &[]).len(), 1);
    }

    #[test]
    fn unmet_requirements_only_checks_accepted_virtual_packages() {
        let spec = |raw: &str| {
            MatchSpec::from_str(raw, rattler_conda_types::ParseStrictness::Lenient).unwrap()
        };
        let host = |microarchitecture: &str| {
            vec![GenericVirtualPackage {
                name: rattler_conda_types::PackageName::try_from("__archspec").unwrap(),
                version: rattler_conda_types::Version::major(1),
                build_string: microarchitecture.to_string(),
            }]
        };

        // A specific host satisfies a baseline requirement...
        assert!(unmet_requirements(&[spec("__archspec 1 x86_64")], &host("zen5")).is_empty());
        // ...including through the CEP-29 regex spelling conda-forge's microarch
        // metapackages use, whose enumeration cannot know future CPUs.
        assert!(
            unmet_requirements(
                &[spec("__archspec 1.* ^(x86_64_v3|skylake)$")],
                &host("zen5")
            )
            .is_empty()
        );
        // A host that reports no microarchitecture at all is not a reason to
        // refuse either.
        assert!(unmet_requirements(&[spec("__archspec 1 x86_64")], &host("0")).is_empty());
        // The same goes for any other name off the list -- pixi has never failed
        // an install over `__unix`, and the fallback path must not either.
        assert!(unmet_requirements(&[spec("__unix")], &[]).is_empty());
        // Every accepted virtual package has its build string compared.
        let cuda = vec![GenericVirtualPackage {
            name: rattler_conda_types::PackageName::try_from("__cuda").unwrap(),
            version: rattler_conda_types::Version::major(12),
            build_string: "real".to_string(),
        }];
        assert_eq!(
            unmet_requirements(&[spec("__cuda 12 other")], &cuda).len(),
            1
        );
    }

    #[test]
    fn test_validate_virtual_packages() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lock_file_path =
            root_dir.join("../../tests/data/lock_files/cuda_virtual_dependency.lock");
        let lock_file = LockFile::from_path(&lock_file_path).unwrap();
        let platform = pixi_manifest::PixiPlatform::from_subdir(Platform::Linux64);

        // Override the virtual package to a version that is not available on the system
        let mut overrides = VirtualPackageOverrides::default();
        overrides.cuda = Some(Override::String("12.0".to_string()));

        let result = validate_system_meets_environment_requirements(
            &lock_file,
            &platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        assert!(result.is_ok(), "{result:?}");

        // Override the virtual package to a version that is not available on the system
        let mut overrides = VirtualPackageOverrides::default();
        overrides.cuda = Some(Override::String("11.0".to_string()));

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
            ),
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
        let mut overrides = VirtualPackageOverrides::default();
        overrides.libc = Some(Override::String("2.28".to_string()));

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

        // To high version for the wheel, which is fine as we assume backwards compatibility
        let mut overrides = VirtualPackageOverrides::default();
        overrides.osx = Some(Override::String("15.1".to_string()));
        overrides.libc = Some(Override::String("2.9999".to_string()));

        let result = validate_system_meets_environment_requirements(
            &lock_file,
            &platform,
            &EnvironmentName::default(),
            Some(overrides),
        );
        assert!(result.is_ok(), "{result:?}");

        // To low version for the wheel
        let mut overrides = VirtualPackageOverrides::default();
        overrides.osx = Some(Override::String("13.0".to_string()));
        overrides.libc = Some(Override::String("2.10".to_string()));

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
        let system_virtual_packages = vec![libc, cuda, osx];

        let error1 =
            VirtualPackageNotFoundError::new(&spec, &system_virtual_packages, Platform::Linux64);

        // Create a test MatchSpec for unix which doesn't have an override
        let spec = MatchSpec::from_str("__unix >= 1.2.3", ParseStrictness::Strict).unwrap();
        let error2 =
            VirtualPackageNotFoundError::new(&spec, &system_virtual_packages, Platform::Linux64);

        assert_snapshot!(format!(
            "With override:\n{}\nWithout override:\n{}",
            format_diagnostic(&error1),
            format_diagnostic(&error2)
        ));
    }
    #[test]
    fn test_virtual_package_not_found_error_with_overrides() {
        // Each override is checked on a target that honors it.
        let overrides = vec![
            (
                "__glibc >= 2.17",
                Platform::Linux64,
                "`CONDA_OVERRIDE_GLIBC=2.17`",
            ),
            (
                "__cuda >= 12.0",
                Platform::Linux64,
                "`CONDA_OVERRIDE_CUDA=12.0`",
            ),
            (
                "__osx >= 10.15",
                Platform::Osx64,
                "`CONDA_OVERRIDE_OSX=10.15`",
            ),
        ];

        let system_virtual_packages: Vec<GenericVirtualPackage> = vec![];

        for (spec, target, msg) in overrides {
            let error = VirtualPackageNotFoundError::new(
                &MatchSpec::from_str(spec, ParseStrictness::Strict).unwrap(),
                &system_virtual_packages,
                target,
            );
            assert!(error.help.unwrap().contains(msg));
        }

        // The same `__osx` requirement on a linux target suggests nothing:
        // CEP 30 has `CONDA_OVERRIDE_OSX` ignored unless the target is `osx-*`.
        let error = VirtualPackageNotFoundError::new(
            &MatchSpec::from_str("__osx >= 10.15", ParseStrictness::Strict).unwrap(),
            &system_virtual_packages,
            Platform::Linux64,
        );
        assert!(error.help.is_none(), "{:?}", error.help);
    }

    #[test]
    fn test_archspec_skip() {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lock_file_path = root_dir.join("../../tests/data/lock_files/archspec.lock");
        let lock_file = LockFile::from_path(&lock_file_path).unwrap();
        let platform = pixi_manifest::PixiPlatform::from_subdir(Platform::Linux64);

        let mut overrides = VirtualPackageOverrides::default();
        overrides.libc = Some(Override::String("2.17".to_string()));

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

        let mut overrides = VirtualPackageOverrides::default();
        overrides.libc = Some(Override::String("2.17".to_string()));
        overrides.cuda = Some(Override::String("11.0".to_string()));

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
