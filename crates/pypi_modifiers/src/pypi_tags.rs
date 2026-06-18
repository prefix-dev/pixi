use miette::Diagnostic;
use pixi_default_versions::{default_glibc_version, default_mac_os_version};
use pixi_manifest::PixiPlatform;
use rattler_conda_types::MatchSpec;
use rattler_conda_types::{
    Arch, GenericVirtualPackage, PackageName, PackageRecord, Platform, Version,
};
use rattler_virtual_packages::VirtualPackage;
use regex::Regex;
use std::str::FromStr;
use std::sync::OnceLock;

#[derive(Debug, thiserror::Error, Diagnostic)]
#[error("failed to determine pypi tags")]
pub enum PyPITagError {
    #[error("failed to determine the python wheel tags for the target platform")]
    FailedToDetermineWheelTags(#[from] uv_platform_tags::TagsError),

    #[error("failed to determine pypi tag for platform: {0}")]
    FailedToDeterminePlatformTags(Platform),

    #[error("failed to determine pypi arch tags for arch: {0}")]
    FailedToDetermineArchTags(Arch),

    #[error("failed to get major and minor version from '{0}' version: '{1}'")]
    FailedToGetMajorMinorVersion(String, String),

    #[error("unsupported libc family: '{0}'")]
    UnsupportedLibCFamily(String),

    #[error("unsupported python implementation: '{0}'")]
    UnsupportedPythonImplementation(String),

    #[error("expected virtual package {0} for {1}.")]
    ExpectedVirtualPackage(String, String),

    #[error("version {0} to high to cast down for platform tag creation")]
    VersionCastError(u64),

    #[error("no tags could be created for platform: {0}")]
    NoTagsForPlatform(String),

    #[error(transparent)]
    ParseMatchSpecError(#[from] rattler_conda_types::ParseMatchSpecError),
}
/// Returns true if the specified record refers to a version/variant of python.
pub fn is_python_record(record: impl AsRef<PackageRecord>) -> bool {
    package_name_is_python(&record.as_ref().name)
}

/// Returns true if the specified package name refers to a version/variant of python.
pub fn is_python_package_name(name: &PackageName) -> bool {
    package_name_is_python(name)
}

/// Returns true if the specified name refers to a version/variant of python.
/// TODO: Add support for more variants.
pub fn package_name_is_python(record: &rattler_conda_types::PackageName) -> bool {
    record.as_normalized() == "python"
}

/// Get the python version and implementation name for the specified platform.
pub fn get_pypi_tags(
    platform: &PixiPlatform,
    python_record: &PackageRecord,
) -> Result<uv_platform_tags::Tags, PyPITagError> {
    let platform = get_platform_tags(platform)?;
    let python_version = get_python_version(python_record)?;
    let implementation_name = get_implementation_name(python_record)?;
    let gil_disabled = gil_disabled(python_record)?;
    create_tags(platform, python_version, implementation_name, gil_disabled)
}

/// Create a uv platform tag for the specified platform
fn get_platform_tags(platform: &PixiPlatform) -> Result<uv_platform_tags::Platform, PyPITagError> {
    let subdir = platform.subdir();
    if subdir.is_linux() {
        get_linux_platform_tags(platform)
    } else if subdir.is_windows() {
        get_windows_platform_tags(platform)
    } else if subdir.is_osx() {
        get_macos_platform_tags(platform)
    } else {
        Err(PyPITagError::FailedToDeterminePlatformTags(subdir))
    }
}

/// Look up a libc-family declaration on `declared`. Returns the conda
/// virtual-package family name (`"glibc"` / `"musl"` / `"eglibc"`) and the
/// declared version.
fn declared_libc(declared: &[GenericVirtualPackage]) -> Option<(&'static str, Version)> {
    declared.iter().find_map(|virtual_package| {
        let family = match virtual_package.name.as_normalized() {
            "__glibc" => "glibc",
            "__musl" => "musl",
            "__eglibc" => "eglibc",
            _ => return None,
        };
        Some((family, virtual_package.version.clone()))
    })
}

/// Look up an exact-name declaration on `declared`.
fn declared_version(declared: &[GenericVirtualPackage], name: &str) -> Option<Version> {
    declared
        .iter()
        .find(|virtual_package| virtual_package.name.as_normalized() == name)
        .map(|virtual_package| virtual_package.version.clone())
}

/// Get linux specific platform tags
fn get_linux_platform_tags(
    platform: &PixiPlatform,
) -> Result<uv_platform_tags::Platform, PyPITagError> {
    let arch = get_arch_tags(platform)?;

    let (family, version) = declared_libc(platform.declared_virtual_packages())
        .unwrap_or_else(|| ("glibc", default_glibc_version()));

    match family {
        "glibc" | "eglibc" => {
            let Some((major, minor)) = version.as_major_minor() else {
                return Err(PyPITagError::FailedToGetMajorMinorVersion(
                    family.to_string(),
                    version.to_string(),
                ));
            };
            Ok(uv_platform_tags::Platform::new(
                uv_platform_tags::Os::Manylinux {
                    major: major as _,
                    minor: minor as _,
                },
                arch,
            ))
        }
        "musl" => {
            let Some((major, minor)) = version.as_major_minor() else {
                return Err(PyPITagError::FailedToGetMajorMinorVersion(
                    "musl".to_string(),
                    version.to_string(),
                ));
            };
            Ok(uv_platform_tags::Platform::new(
                uv_platform_tags::Os::Musllinux {
                    major: major as _,
                    minor: minor as _,
                },
                arch,
            ))
        }
        other => Err(PyPITagError::UnsupportedLibCFamily(other.to_string())),
    }
}

/// Get windows specific platform tags
fn get_windows_platform_tags(
    platform: &PixiPlatform,
) -> Result<uv_platform_tags::Platform, PyPITagError> {
    let arch = get_arch_tags(platform)?;
    Ok(uv_platform_tags::Platform::new(
        uv_platform_tags::Os::Windows,
        arch,
    ))
}

/// Resolve the macOS version pixi targets for `platform`: the declared `__osx`
/// virtual package (from `[system-requirements] macos`), falling back to the
/// channel default for the subdir. Callers must ensure `platform` is macOS --
/// the fallback [`default_mac_os_version`] panics otherwise.
fn macos_target_version(platform: &PixiPlatform) -> Version {
    declared_version(platform.declared_virtual_packages(), "__osx")
        .unwrap_or_else(|| default_mac_os_version(platform.subdir()))
}

/// Get macos specific platform tags
fn get_macos_platform_tags(
    platform: &PixiPlatform,
) -> Result<uv_platform_tags::Platform, PyPITagError> {
    let (major, minor) = macos_major_minor(&macos_target_version(platform), "macos")?;

    let arch = get_arch_tags(platform)?;

    Ok(uv_platform_tags::Platform::new(
        uv_platform_tags::Os::Macos {
            major: major as _,
            minor: minor as _,
        },
        arch,
    ))
}

/// The macOS deployment target (`"<major>.<minor>"`) pixi targets for
/// `platform`, or `None` when the target platform is not macOS.
///
/// Pixi exports this as `MACOSX_DEPLOYMENT_TARGET` when building PyPI packages
/// from an sdist so the produced wheel is tagged with the same macOS version the
/// resolver targets. CMake-based build backends (e.g. scikit-build-core)
/// otherwise default the deployment target to the *building* machine's macOS
/// version, producing a wheel tag uv rejects as incompatible with the resolved
/// target (see [`get_macos_platform_tags`]).
pub fn macos_deployment_target(platform: &PixiPlatform) -> Option<String> {
    if !platform.subdir().is_osx() {
        return None;
    }
    let (major, minor) = macos_major_minor(&macos_target_version(platform), "macos").ok()?;
    Some(format!("{major}.{minor}"))
}

/// Single-segment fallback for [`Version::as_major_minor`]: returns the
/// numeric major when the version has exactly one segment (e.g.
/// `macos = "15"`), so the caller can default the minor to 0.
fn major_only(version: &Version) -> Option<u64> {
    version.segments().next()?.components().next()?.as_number()
}

/// Extract a macOS `(major, minor)` from `version`, accepting a single-segment
/// value (`15` -> `(15, 0)`) via [`major_only`]. `label` names the source in
/// the error. Shared so every macOS tag path applies the same fallback.
fn macos_major_minor(version: &Version, label: &str) -> Result<(u64, u64), PyPITagError> {
    version
        .as_major_minor()
        .or_else(|| Some((major_only(version)?, 0)))
        .ok_or_else(|| {
            PyPITagError::FailedToGetMajorMinorVersion(label.to_string(), version.to_string())
        })
}

/// Get the arch tag for the specified platform
fn get_arch_tags(platform: &PixiPlatform) -> Result<uv_platform_tags::Arch, PyPITagError> {
    match platform.subdir().arch() {
        None => unreachable!("every platform we support has an arch"),
        Some(Arch::X86) => Ok(uv_platform_tags::Arch::X86),
        Some(Arch::X86_64) => Ok(uv_platform_tags::Arch::X86_64),
        Some(Arch::Aarch64 | Arch::Arm64) => Ok(uv_platform_tags::Arch::Aarch64),
        Some(Arch::ArmV7l) => Ok(uv_platform_tags::Arch::Armv7L),
        Some(Arch::Ppc64le) => Ok(uv_platform_tags::Arch::Powerpc64Le),
        Some(Arch::Ppc64) => Ok(uv_platform_tags::Arch::Powerpc64),
        Some(Arch::Riscv64) => Ok(uv_platform_tags::Arch::Riscv64),
        Some(Arch::S390X) => Ok(uv_platform_tags::Arch::S390X),
        Some(Arch::LoongArch64) => Ok(uv_platform_tags::Arch::LoongArch64),
        Some(unsupported_arch) => Err(PyPITagError::FailedToDetermineArchTags(unsupported_arch)),
    }
}

fn get_python_version(python_record: &PackageRecord) -> Result<(u8, u8), PyPITagError> {
    let Some(python_version) = python_record.version.as_major_minor() else {
        return Err(PyPITagError::FailedToGetMajorMinorVersion(
            python_record.name.as_normalized().to_string(),
            python_record.version.to_string(),
        ));
    };
    Ok((python_version.0 as u8, python_version.1 as u8))
}

fn get_implementation_name(python_record: &PackageRecord) -> Result<&'static str, PyPITagError> {
    match python_record.name.as_normalized() {
        "python" => Ok("cpython"),
        "pypy" => Ok("pypy"),
        _ => Err(PyPITagError::UnsupportedPythonImplementation(
            python_record.name.as_normalized().to_string(),
        )),
    }
}

/// Return whether the specified record has gil disabled (by being a free-threaded python interpreter)
fn gil_disabled(python_record: &PackageRecord) -> Result<bool, PyPITagError> {
    // In order to detect if the python interpreter is free-threaded, we look at the depends
    // field of the record. If the record has a dependency on `python_abi`, then
    // look at the build string to detect cpXXXt (free-threaded python interpreter).
    static REGEX: OnceLock<Regex> = OnceLock::new();

    let regex = REGEX.get_or_init(|| {
        Regex::new(r"cp\d{3}t").expect("regex for free-threaded python interpreter should compile")
    });

    let deps = python_record
        .depends
        .iter()
        .map(|dep| MatchSpec::from_str(dep, rattler_conda_types::ParseStrictness::Lenient))
        .collect::<Result<Vec<MatchSpec>, _>>()?;

    let python_abi =
        PackageName::from_str("python_abi").expect("python_abi is a valid package name");
    Ok(deps.iter().any(|spec| {
        spec.name.matches(&python_abi)
            && spec.build.as_ref().is_some_and(|build| {
                let raw_str = format!("{build}");
                regex.is_match(&raw_str)
            })
    }))
}

/// Create the pypi tags for the specified platform, python version, and implementation name
fn create_tags(
    platform: uv_platform_tags::Platform,
    python_version: (u8, u8),
    implementation_name: &str,
    gil_disabled: bool,
) -> Result<uv_platform_tags::Tags, PyPITagError> {
    uv_platform_tags::Tags::from_env(
        &platform,
        python_version,
        implementation_name,
        // TODO: This might not be entirely correct..
        python_version,
        uv_platform_tags::TagsOptions {
            manylinux_compatible: true,
            gil_disabled,
            ..Default::default()
        },
    )
    .map_err(PyPITagError::FailedToDetermineWheelTags)
}

/// Get the pypi platform from the conda virtual packages
/// Used to get the platform for the environment validation in the lock file.
fn get_pypi_platform_from_virtual_packages(
    virtual_packages: &[VirtualPackage],
    platform: &PixiPlatform,
) -> Result<uv_platform_tags::Platform, PyPITagError> {
    let subdir = platform.subdir();
    if subdir.is_linux() {
        // The linux platform is mostly based on the libc version
        let libc = virtual_packages
            .iter()
            .find_map(|package| match package {
                VirtualPackage::LibC(libc) => Some(libc),
                _ => None,
            })
            .ok_or(PyPITagError::ExpectedVirtualPackage(
                "libc".to_string(),
                platform.to_string(),
            ))?;

        let (major, minor) =
            libc.version
                .as_major_minor()
                .ok_or(PyPITagError::FailedToGetMajorMinorVersion(
                    "libc".to_string(),
                    libc.version.to_string(),
                ))?;
        // Protect casting with an error to avoid hard to find bugs
        let major = u64::try_into(major).map_err(|_| PyPITagError::VersionCastError(major))?;
        let minor = u64::try_into(minor).map_err(|_| PyPITagError::VersionCastError(minor))?;

        return match libc.family.to_lowercase().as_str() {
            pixi_manifest::GLIBC_FAMILY => Ok(uv_platform_tags::Platform::new(
                uv_platform_tags::Os::Manylinux { major, minor },
                get_arch_tags(platform)?,
            )),
            pixi_manifest::MUSL_FAMILY => Ok(uv_platform_tags::Platform::new(
                uv_platform_tags::Os::Musllinux { major, minor },
                get_arch_tags(platform)?,
            )),
            // TODO: Add more libc families for support of other linux distributions
            _ => Err(PyPITagError::UnsupportedLibCFamily(libc.to_string())),
        };
    }

    if subdir.is_windows() {
        return Ok(uv_platform_tags::Platform::new(
            uv_platform_tags::Os::Windows,
            get_arch_tags(platform)?,
        ));
    }

    if subdir.is_osx() {
        let osx = virtual_packages
            .iter()
            .find_map(|package| match package {
                VirtualPackage::Osx(osx) => Some(osx),
                _ => None,
            })
            .ok_or(PyPITagError::ExpectedVirtualPackage(
                "osx".to_string(),
                platform.to_string(),
            ))?;

        let (major, minor) = macos_major_minor(&osx.version, &platform.to_string())?;
        // Protect casting with an error to avoid hard to find bugs
        let major = u64::try_into(major).map_err(|_| PyPITagError::VersionCastError(major))?;
        let minor = u64::try_into(minor).map_err(|_| PyPITagError::VersionCastError(minor))?;

        return Ok(uv_platform_tags::Platform::new(
            uv_platform_tags::Os::Macos { major, minor },
            get_arch_tags(platform)?,
        ));
    }

    Err(PyPITagError::NoTagsForPlatform(platform.to_string()))
}

/// Get the pypi tags for this machine and the given python record
/// Designed to work for the environment validation in the lock file with the current machine.
pub fn get_tags_from_machine(
    virtual_packages: &[VirtualPackage],
    platform: &PixiPlatform,
    python_record: &PackageRecord,
) -> Result<uv_platform_tags::Tags, PyPITagError> {
    let platform = get_pypi_platform_from_virtual_packages(virtual_packages, platform)?;
    create_tags(
        platform,
        get_python_version(python_record)?,
        get_implementation_name(python_record)?,
        gil_disabled(python_record)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_conda_types::VersionWithSource;
    use rattler_virtual_packages::{LibC, Osx};
    use std::str::FromStr;
    use uv_distribution_filename::WheelFilename;
    use uv_platform_tags::Arch as UvArch;

    #[test]
    fn test_get_platform_from_vpkgs_osx() {
        let vpkgs = vec![VirtualPackage::Osx(Osx {
            version: "15.1.0".parse().unwrap(),
        })];
        let platform = Platform::OsxArm64;
        let res =
            get_pypi_platform_from_virtual_packages(&vpkgs, &PixiPlatform::from_subdir(platform));
        let platform = res.unwrap();
        assert_eq!(
            platform.os(),
            &uv_platform_tags::Os::Macos {
                major: 15,
                minor: 1
            }
        );
        assert_eq!(platform.arch(), UvArch::Aarch64);

        let vpkgs = vec![VirtualPackage::Osx(Osx {
            version: "12.1.0".parse().unwrap(),
        })];
        let platform = Platform::Osx64;
        let res =
            get_pypi_platform_from_virtual_packages(&vpkgs, &PixiPlatform::from_subdir(platform));
        let platform = res.unwrap();
        assert_eq!(
            platform.os(),
            &uv_platform_tags::Os::Macos {
                major: 12,
                minor: 1
            }
        );
        assert_eq!(platform.arch(), UvArch::X86_64);
    }

    /// A single-segment `__osx` version (`15`) must resolve through the
    /// virtual-package path too, not just `get_macos_platform_tags`.
    #[test]
    fn test_get_platform_from_vpkgs_osx_major_only() {
        let vpkgs = vec![VirtualPackage::Osx(Osx {
            version: "15".parse().unwrap(),
        })];
        let res = get_pypi_platform_from_virtual_packages(
            &vpkgs,
            &PixiPlatform::from_subdir(Platform::OsxArm64),
        );
        assert_eq!(
            res.unwrap().os(),
            &uv_platform_tags::Os::Macos {
                major: 15,
                minor: 0
            }
        );
    }

    #[test]
    fn test_macos_deployment_target() {
        // A declared `__osx` (from `[system-requirements] macos`) wins.
        let platform = PixiPlatform::from_required_virtual_packages(
            Platform::OsxArm64,
            vec![GenericVirtualPackage {
                name: "__osx".parse().unwrap(),
                version: "12.0".parse().unwrap(),
                build_string: "0".to_string(),
            }],
        );
        assert_eq!(macos_deployment_target(&platform), Some("12.0".to_string()));

        // A single-segment declaration gets a `.0` minor.
        let platform = PixiPlatform::from_required_virtual_packages(
            Platform::OsxArm64,
            vec![GenericVirtualPackage {
                name: "__osx".parse().unwrap(),
                version: "15".parse().unwrap(),
                build_string: "0".to_string(),
            }],
        );
        assert_eq!(macos_deployment_target(&platform), Some("15.0".to_string()));

        // No declaration falls back to the subdir default.
        assert_eq!(
            macos_deployment_target(&PixiPlatform::from_subdir(Platform::OsxArm64)),
            Some("13.0".to_string())
        );

        // Non-macOS targets get nothing.
        assert_eq!(
            macos_deployment_target(&PixiPlatform::from_subdir(Platform::Linux64)),
            None
        );
        assert_eq!(
            macos_deployment_target(&PixiPlatform::from_subdir(Platform::Win64)),
            None
        );
    }

    #[test]
    fn test_get_platform_from_vpgks_linux() {
        let vpkgs = vec![VirtualPackage::LibC(LibC {
            family: "glibc".to_string(),
            version: "2.33".parse().unwrap(),
        })];
        let platform = Platform::Linux64;
        let res =
            get_pypi_platform_from_virtual_packages(&vpkgs, &PixiPlatform::from_subdir(platform));
        let platform = res.unwrap();
        assert_eq!(
            platform.os(),
            &uv_platform_tags::Os::Manylinux {
                major: 2,
                minor: 33
            }
        );
        assert_eq!(platform.arch(), UvArch::X86_64);

        let vpkgs = vec![VirtualPackage::LibC(LibC {
            family: "musl".to_string(),
            version: "1.2".parse().unwrap(),
        })];
        let platform = Platform::Linux64;
        let res =
            get_pypi_platform_from_virtual_packages(&vpkgs, &PixiPlatform::from_subdir(platform));
        let platform = res.unwrap();
        assert_eq!(
            platform.os(),
            &uv_platform_tags::Os::Musllinux { major: 1, minor: 2 }
        );
        assert_eq!(platform.arch(), UvArch::X86_64);

        let platform = Platform::LinuxAarch64;
        let res =
            get_pypi_platform_from_virtual_packages(&vpkgs, &PixiPlatform::from_subdir(platform));
        let platform = res.unwrap();
        assert_eq!(
            platform.os(),
            &uv_platform_tags::Os::Musllinux { major: 1, minor: 2 }
        );
        assert_eq!(platform.arch(), UvArch::Aarch64);

        let vpkgs = vec![VirtualPackage::LibC(LibC {
            family: "musl".to_string(),
            version: "1.2".parse().unwrap(),
        })];
        let platform = Platform::LinuxPpc64le;
        let res =
            get_pypi_platform_from_virtual_packages(&vpkgs, &PixiPlatform::from_subdir(platform));
        let platform = res.unwrap();
        assert_eq!(
            platform.os(),
            &uv_platform_tags::Os::Musllinux { major: 1, minor: 2 }
        );
        assert_eq!(platform.arch(), UvArch::Powerpc64Le);
    }

    #[test]
    fn test_get_platform_from_vpkgs_windows() {
        let vpkgs = vec![];
        let platform = Platform::Win64;
        let res =
            get_pypi_platform_from_virtual_packages(&vpkgs, &PixiPlatform::from_subdir(platform));
        let platform = res.unwrap();
        assert_eq!(platform.os(), &uv_platform_tags::Os::Windows);
        assert_eq!(platform.arch(), UvArch::X86_64);

        let platform = Platform::WinArm64;
        let res =
            get_pypi_platform_from_virtual_packages(&vpkgs, &PixiPlatform::from_subdir(platform));
        let platform = res.unwrap();
        assert_eq!(platform.os(), &uv_platform_tags::Os::Windows);
        assert_eq!(platform.arch(), UvArch::Aarch64);
    }

    #[test]
    fn test_get_platform_from_vpkgs_error() {
        // No virtual packages gives an error
        let vpkgs = vec![];
        let platform = Platform::Linux64;
        let res =
            get_pypi_platform_from_virtual_packages(&vpkgs, &PixiPlatform::from_subdir(platform));
        assert!(res.is_err());

        // Unknown libc family gives an error
        let vpkgs = vec![VirtualPackage::LibC(LibC {
            family: "unknown".to_string(),
            version: "1.2".parse().unwrap(),
        })];
        let platform = Platform::Linux64;
        let res =
            get_pypi_platform_from_virtual_packages(&vpkgs, &PixiPlatform::from_subdir(platform));
        assert!(res.is_err());
        assert!(matches!(
            res.unwrap_err(),
            PyPITagError::UnsupportedLibCFamily(_)
        ));
    }

    #[test]
    fn test_tags_from_linux_machine() {
        // Linux
        let vpkgs = vec![VirtualPackage::LibC(LibC {
            family: "glibc".to_string(),
            version: "2.33".parse().unwrap(),
        })];
        let platform = Platform::Linux64;
        let python_record = PackageRecord::new(
            "python".parse().unwrap(),
            VersionWithSource::from_str("3.13.3").unwrap(),
            "h2334245_104_cp313".to_string(),
        );
        let res =
            get_tags_from_machine(&vpkgs, &PixiPlatform::from_subdir(platform), &python_record)
                .unwrap();

        let wheel =
            WheelFilename::from_str("numpy-1.21.0-cp313-cp313-manylinux_2_33_x86_64.whl").unwrap();
        assert!(wheel.is_compatible(&res));

        let wheel = WheelFilename::from_str("osm2geojson-0.2.4-py3-none-any.whl").unwrap();
        assert!(wheel.is_compatible(&res));

        let wheel =
            WheelFilename::from_str("charset_normalizer-3.3.2-cp312-cp312-macosx_10_9_x86_64.whl")
                .unwrap();
        assert!(!wheel.is_compatible(&res));
    }

    #[test]
    fn test_tags_from_macos_machine() {
        let vpkgs = vec![VirtualPackage::Osx(Osx {
            version: "15.1.0".parse().unwrap(),
        })];
        let platform = Platform::OsxArm64;
        let python_record = PackageRecord::new(
            "python".parse().unwrap(),
            VersionWithSource::from_str("3.13.3").unwrap(),
            "h2334245_104_cp313".to_string(),
        );
        let res =
            get_tags_from_machine(&vpkgs, &PixiPlatform::from_subdir(platform), &python_record)
                .unwrap();

        let wheel =
            WheelFilename::from_str("numpy-1.21.0-cp313-cp313-macosx_15_0_arm64.whl").unwrap();
        assert!(wheel.is_compatible(&res));

        let wheel = WheelFilename::from_str("osm2geojson-0.2.4-py3-none-any.whl").unwrap();
        assert!(wheel.is_compatible(&res));

        let wheel =
            WheelFilename::from_str("charset_normalizer-3.3.2-cp312-cp312-macosx_10_9_x86_64.whl")
                .unwrap();
        assert!(!wheel.is_compatible(&res));
    }

    #[test]
    fn test_tags_from_windows_machine() {
        let vpkgs = vec![];
        let platform = Platform::Win64;
        let python_record = PackageRecord::new(
            "python".parse().unwrap(),
            VersionWithSource::from_str("3.13.3").unwrap(),
            "h2334245_104_cp313".to_string(),
        );
        let res =
            get_tags_from_machine(&vpkgs, &PixiPlatform::from_subdir(platform), &python_record)
                .unwrap();

        let wheel = WheelFilename::from_str("numpy-1.21.0-cp313-cp313-win_amd64.whl").unwrap();
        assert!(wheel.is_compatible(&res));

        let wheel = WheelFilename::from_str("all-0.2.4-py3-none-any.whl").unwrap();
        assert!(wheel.is_compatible(&res));

        let wheel = WheelFilename::from_str("not_windows-3.3.2-cp312-cp312-macosx_10_9_x86_64.whl")
            .unwrap();
        assert!(!wheel.is_compatible(&res));
    }

    fn rich_platform(
        name: &str,
        subdir: Platform,
        declared: Vec<GenericVirtualPackage>,
    ) -> PixiPlatform {
        PixiPlatform::new(
            pixi_manifest::PixiPlatformName::try_from(name).unwrap(),
            subdir,
            declared,
        )
        .expect("test inputs respect the subdir-platform invariant")
    }

    fn declared(name: &str, version: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: PackageName::try_from(name).unwrap(),
            version: Version::from_str(version).unwrap(),
            build_string: String::new(),
        }
    }

    /// A platform declaring `__musl` produces a `Musllinux` tag, with the
    /// family and version read straight from the declaration.
    #[test]
    fn linux_tag_reads_musl_from_platform() {
        let platform = rich_platform(
            "alpine",
            Platform::LinuxAarch64,
            vec![declared("__musl", "1.2.4")],
        );
        let res = get_linux_platform_tags(&platform).unwrap();
        assert_eq!(
            res.os(),
            &uv_platform_tags::Os::Musllinux { major: 1, minor: 2 }
        );
    }

    /// A platform declaring `__glibc` produces a `Manylinux` tag at the
    /// declared version (not the default).
    #[test]
    fn linux_tag_reads_glibc_from_platform() {
        let platform = rich_platform(
            "modern-linux",
            Platform::Linux64,
            vec![declared("__glibc", "2.36")],
        );
        let res = get_linux_platform_tags(&platform).unwrap();
        assert_eq!(
            res.os(),
            &uv_platform_tags::Os::Manylinux {
                major: 2,
                minor: 36
            }
        );
    }

    /// Without a libc declaration the linux tag falls back to the project's
    /// default glibc version.
    #[test]
    fn linux_tag_falls_back_to_default_glibc() {
        let platform = PixiPlatform::from_subdir(Platform::Linux64);
        let res = get_linux_platform_tags(&platform).unwrap();
        let (default_major, default_minor) = default_glibc_version()
            .as_major_minor()
            .expect("default glibc has major/minor");
        assert_eq!(
            res.os(),
            &uv_platform_tags::Os::Manylinux {
                major: default_major as _,
                minor: default_minor as _
            }
        );
    }

    /// A platform declaring `__osx` produces a macOS tag at the declared
    /// version (not the subdir's default).
    #[test]
    fn macos_tag_reads_osx_from_platform() {
        let platform = rich_platform(
            "modern-mac",
            Platform::OsxArm64,
            vec![declared("__osx", "14.0")],
        );
        let res = get_macos_platform_tags(&platform).unwrap();
        assert_eq!(
            res.os(),
            &uv_platform_tags::Os::Macos {
                major: 14,
                minor: 0
            }
        );
    }

    /// A single-segment macOS version (`macos = "15"`) is accepted with the
    /// minor defaulting to 0, instead of failing the pypi-tag build.
    #[test]
    fn macos_tag_accepts_major_only_version() {
        let platform = rich_platform(
            "macos-15",
            Platform::OsxArm64,
            vec![declared("__osx", "15")],
        );
        let res = get_macos_platform_tags(&platform).unwrap();
        assert_eq!(
            res.os(),
            &uv_platform_tags::Os::Macos {
                major: 15,
                minor: 0
            }
        );
    }
}
