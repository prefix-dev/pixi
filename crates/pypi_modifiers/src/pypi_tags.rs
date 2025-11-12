use miette::Diagnostic;
use pixi_default_versions::{default_glibc_version, default_mac_os_version};
use pixi_manifest::{LibCSystemRequirement, SystemRequirements};
use rattler_conda_types::MatchSpec;
use rattler_conda_types::{Arch, PackageRecord, Platform};
use rattler_virtual_packages::VirtualPackage;
use regex::Regex;
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

/// Returns true if the specified name refers to a version/variant of python.
/// TODO: Add support for more variants.
pub fn package_name_is_python(record: &rattler_conda_types::PackageName) -> bool {
    record.as_normalized() == "python"
}

/// Get the python version and implementation name for the specified platform.
pub fn get_pypi_tags(
    platform: Platform,
    system_requirements: &SystemRequirements,
    python_record: &PackageRecord,
) -> Result<uv_platform_tags::Tags, PyPITagError> {
    let platform = get_platform_tags(platform, system_requirements)?;
    let python_version = get_python_version(python_record)?;
    let implementation_name = get_implementation_name(python_record)?;
    let gil_disabled = gil_disabled(python_record)?;
    create_tags(platform, python_version, implementation_name, gil_disabled)
}

/// Create a uv platform tag for the specified platform
fn get_platform_tags(
    platform: Platform,
    system_requirements: &SystemRequirements,
) -> Result<uv_platform_tags::Platform, PyPITagError> {
    if platform.is_linux() {
        get_linux_platform_tags(platform, system_requirements)
    } else if platform.is_windows() {
        get_windows_platform_tags(platform)
    } else if platform.is_osx() {
        get_macos_platform_tags(platform, system_requirements)
    } else {
        Err(PyPITagError::FailedToDeterminePlatformTags(platform))
    }
}

/// Get linux specific platform tags
fn get_linux_platform_tags(
    platform: Platform,
    system_requirements: &SystemRequirements,
) -> Result<uv_platform_tags::Platform, PyPITagError> {
    let arch = get_arch_tags(platform)?;

    // Find the glibc version
    match system_requirements
        .libc
        .as_ref()
        .map(LibCSystemRequirement::family_and_version)
    {
        None => {
            let (major, minor) = default_glibc_version()
                .as_major_minor()
                .expect("default glibc version should be valid");
            Ok(uv_platform_tags::Platform::new(
                uv_platform_tags::Os::Manylinux {
                    major: major as _,
                    minor: minor as _,
                },
                arch,
            ))
        }
        Some(("glibc", version)) => {
            let Some((major, minor)) = version.as_major_minor() else {
                return Err(PyPITagError::FailedToGetMajorMinorVersion(
                    "glibc".to_string(),
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
        Some(("musl", version)) => {
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
        Some((family, _)) => Err(PyPITagError::UnsupportedLibCFamily(family.to_string())),
    }
}

/// Get windows specific platform tags
fn get_windows_platform_tags(
    platform: Platform,
) -> Result<uv_platform_tags::Platform, PyPITagError> {
    let arch = get_arch_tags(platform)?;
    Ok(uv_platform_tags::Platform::new(
        uv_platform_tags::Os::Windows,
        arch,
    ))
}

/// Get macos specific platform tags
fn get_macos_platform_tags(
    platform: Platform,
    system_requirements: &SystemRequirements,
) -> Result<uv_platform_tags::Platform, PyPITagError> {
    let osx_version = system_requirements
        .macos
        .clone()
        .unwrap_or_else(|| default_mac_os_version(platform));
    let Some((major, minor)) = osx_version.as_major_minor() else {
        return Err(PyPITagError::FailedToGetMajorMinorVersion(
            "macos".to_string(),
            osx_version.to_string(),
        ));
    };

    let arch = get_arch_tags(platform)?;

    Ok(uv_platform_tags::Platform::new(
        uv_platform_tags::Os::Macos {
            major: major as _,
            minor: minor as _,
        },
        arch,
    ))
}

/// Get the arch tag for the specified platform
fn get_arch_tags(platform: Platform) -> Result<uv_platform_tags::Arch, PyPITagError> {
    match platform.arch() {
        None => unreachable!("every platform we support has an arch"),
        Some(Arch::X86) => Ok(uv_platform_tags::Arch::X86),
        Some(Arch::X86_64) => Ok(uv_platform_tags::Arch::X86_64),
        Some(Arch::Aarch64 | Arch::Arm64) => Ok(uv_platform_tags::Arch::Aarch64),
        Some(Arch::ArmV7l) => Ok(uv_platform_tags::Arch::Armv7L),
        Some(Arch::Ppc64le) => Ok(uv_platform_tags::Arch::Powerpc64Le),
        Some(Arch::Ppc64) => Ok(uv_platform_tags::Arch::Powerpc64),
        Some(Arch::Riscv64) => Ok(uv_platform_tags::Arch::Riscv64),
        Some(Arch::S390X) => Ok(uv_platform_tags::Arch::S390X),
        Some(Arch::Loong64) => Ok(uv_platform_tags::Arch::LoongArch64),
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

    Ok(deps.iter().any(|spec| {
        spec.name
            .as_ref()
            .is_some_and(|name| name.as_source() == "python_abi")
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
        true,
        gil_disabled,
    )
    .map_err(PyPITagError::FailedToDetermineWheelTags)
}

/// Get the pypi platform from the conda virtual packages
/// Used to get the platform for the environment validation in the lock file.
fn get_pypi_platform_from_virtual_packages(
    virtual_packages: &[VirtualPackage],
    platform: Platform,
) -> Result<uv_platform_tags::Platform, PyPITagError> {
    if platform.is_linux() {
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

    if platform.is_windows() {
        return Ok(uv_platform_tags::Platform::new(
            uv_platform_tags::Os::Windows,
            get_arch_tags(platform)?,
        ));
    }

    if platform.is_osx() {
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

        let (major, minor) =
            osx.version
                .as_major_minor()
                .ok_or(PyPITagError::FailedToGetMajorMinorVersion(
                    platform.to_string(),
                    osx.version.to_string(),
                ))?;
        // Protect casting with an error to avoid hard to find bugs
        let major = u64::try_into(major).map_err(|_| PyPITagError::VersionCastError(major))?;
        let minor = u64::try_into(minor).map_err(|_| PyPITagError::VersionCastError(minor))?;

        return Ok(uv_platform_tags::Platform::new(
            uv_platform_tags::Os::Macos { major, minor },
            get_arch_tags(platform.to_owned())?,
        ));
    }

    Err(PyPITagError::NoTagsForPlatform(platform.to_string()))
}

/// Get the pypi tags for this machine and the given python record
/// Designed to work for the environment validation in the lock file with the current machine.
pub fn get_tags_from_machine(
    virtual_packages: &[VirtualPackage],
    platform: Platform,
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
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
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
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
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

    #[test]
    fn test_get_platform_from_vpgks_linux() {
        let vpkgs = vec![VirtualPackage::LibC(LibC {
            family: "glibc".to_string(),
            version: "2.33".parse().unwrap(),
        })];
        let platform = Platform::Linux64;
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
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
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
        let platform = res.unwrap();
        assert_eq!(
            platform.os(),
            &uv_platform_tags::Os::Musllinux { major: 1, minor: 2 }
        );
        assert_eq!(platform.arch(), UvArch::X86_64);

        let platform = Platform::LinuxAarch64;
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
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
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
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
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
        let platform = res.unwrap();
        assert_eq!(platform.os(), &uv_platform_tags::Os::Windows);
        assert_eq!(platform.arch(), UvArch::X86_64);

        let platform = Platform::WinArm64;
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
        let platform = res.unwrap();
        assert_eq!(platform.os(), &uv_platform_tags::Os::Windows);
        assert_eq!(platform.arch(), UvArch::Aarch64);
    }

    #[test]
    fn test_get_platform_from_vpkgs_error() {
        // No virtual packages gives an error
        let vpkgs = vec![];
        let platform = Platform::Linux64;
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
        assert!(res.is_err());

        // Unknown libc family gives an error
        let vpkgs = vec![VirtualPackage::LibC(LibC {
            family: "unknown".to_string(),
            version: "1.2".parse().unwrap(),
        })];
        let platform = Platform::Linux64;
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
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
        let res = get_tags_from_machine(&vpkgs, platform, &python_record).unwrap();

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
        let res = get_tags_from_machine(&vpkgs, platform, &python_record).unwrap();

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
        let res = get_tags_from_machine(&vpkgs, platform, &python_record).unwrap();

        let wheel = WheelFilename::from_str("numpy-1.21.0-cp313-cp313-win_amd64.whl").unwrap();
        assert!(wheel.is_compatible(&res));

        let wheel = WheelFilename::from_str("all-0.2.4-py3-none-any.whl").unwrap();
        assert!(wheel.is_compatible(&res));

        let wheel = WheelFilename::from_str("not_windows-3.3.2-cp312-cp312-macosx_10_9_x86_64.whl")
            .unwrap();
        assert!(!wheel.is_compatible(&res));
    }
}
