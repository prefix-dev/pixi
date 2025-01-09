use std::sync::OnceLock;

use miette::{Context, Diagnostic, IntoDiagnostic};
use pixi_default_versions::{default_glibc_version, default_mac_os_version};
use pixi_manifest::{LibCSystemRequirement, SystemRequirements};
use rattler_conda_types::MatchSpec;
use rattler_conda_types::{Arch, PackageRecord, Platform};
use rattler_virtual_packages::VirtualPackage;
use regex::Regex;
use uv_platform_tags::Os as UvOs;
use uv_platform_tags::Tags;

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
) -> miette::Result<Tags> {
    let platform = get_platform_tags(platform, system_requirements)?;
    let python_version = get_python_version(python_record).into_diagnostic()?;
    let implementation_name = get_implementation_name(python_record).into_diagnostic()?;
    let gil_disabled = gil_disabled(python_record)?;
    create_tags(platform, python_version, implementation_name, gil_disabled)
}

/// Create a uv platform tag for the specified platform
fn get_platform_tags(
    platform: Platform,
    system_requirements: &SystemRequirements,
) -> miette::Result<uv_platform_tags::Platform> {
    if platform.is_linux() {
        get_linux_platform_tags(platform, system_requirements)
    } else if platform.is_windows() {
        get_windows_platform_tags(platform)
    } else if platform.is_osx() {
        get_macos_platform_tags(platform, system_requirements)
    } else {
        miette::bail!("unsupported platform for pypi packages {platform}")
    }
}

/// Get linux specific platform tags
fn get_linux_platform_tags(
    platform: Platform,
    system_requirements: &SystemRequirements,
) -> miette::Result<uv_platform_tags::Platform> {
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
                .expect("expected default glibc version to be a major.minor version");
            Ok(uv_platform_tags::Platform::new(
                UvOs::Manylinux {
                    major: major as _,
                    minor: minor as _,
                },
                arch,
            ))
        }
        Some(("glibc", version)) => {
            let Some((major, minor)) = version.as_major_minor() else {
                miette::bail!(
                    "expected glibc version to be a major.minor version, but got '{version}'"
                )
            };
            Ok(uv_platform_tags::Platform::new(
                UvOs::Manylinux {
                    major: major as _,
                    minor: minor as _,
                },
                arch,
            ))
        }
        Some((family, _)) => {
            miette::bail!("unsupported libc family for pypi packages '{family}'");
        }
    }
}

/// Get windows specific platform tags
fn get_windows_platform_tags(platform: Platform) -> miette::Result<uv_platform_tags::Platform> {
    let arch = get_arch_tags(platform)?;
    Ok(uv_platform_tags::Platform::new(UvOs::Windows, arch))
}

/// Get macos specific platform tags
fn get_macos_platform_tags(
    platform: Platform,
    system_requirements: &SystemRequirements,
) -> miette::Result<uv_platform_tags::Platform> {
    let osx_version = system_requirements
        .macos
        .clone()
        .unwrap_or_else(|| default_mac_os_version(platform));
    let Some((major, minor)) = osx_version.as_major_minor() else {
        miette::bail!("expected macos version to be a major.minor version, but got '{osx_version}'")
    };

    let arch = get_arch_tags(platform)?;

    Ok(uv_platform_tags::Platform::new(
        UvOs::Macos {
            major: major as _,
            minor: minor as _,
        },
        arch,
    ))
}

#[derive(Debug, thiserror::Error, Diagnostic)]
#[error("unsupported architecture for pypi tags: {0}")]
pub struct ArchTagsError(Arch);
/// Get the arch tag for the specified platform
fn get_arch_tags(platform: Platform) -> Result<uv_platform_tags::Arch, ArchTagsError> {
    match platform.arch() {
        None => unreachable!("every platform we support has an arch"),
        Some(Arch::X86) => Ok(uv_platform_tags::Arch::X86),
        Some(Arch::X86_64) => Ok(uv_platform_tags::Arch::X86_64),
        Some(Arch::Aarch64 | Arch::Arm64) => Ok(uv_platform_tags::Arch::Aarch64),
        Some(Arch::ArmV7l) => Ok(uv_platform_tags::Arch::Armv7L),
        Some(Arch::Ppc64le) => Ok(uv_platform_tags::Arch::Powerpc64Le),
        Some(Arch::Ppc64) => Ok(uv_platform_tags::Arch::Powerpc64),
        Some(Arch::S390X) => Ok(uv_platform_tags::Arch::S390X),
        Some(unsupported_arch) => Err(ArchTagsError(unsupported_arch)),
    }
}

#[derive(Debug, thiserror::Error, Diagnostic)]
#[error("expected python version to be a major.minor version, but got '{0}'")]
pub struct PythonVersionTagsError(String);
fn get_python_version(python_record: &PackageRecord) -> Result<(u8, u8), PythonVersionTagsError> {
    let Some(python_version) = python_record.version.as_major_minor() else {
        return Err(PythonVersionTagsError(python_record.version.to_string()));
    };
    Ok((python_version.0 as u8, python_version.1 as u8))
}

#[derive(Debug, thiserror::Error, Diagnostic)]
#[error("unsupported python implementation: '{0}'")]
pub struct UnsupportedPythonImplementationError(String);
fn get_implementation_name(
    python_record: &PackageRecord,
) -> Result<&'static str, UnsupportedPythonImplementationError> {
    match python_record.name.as_normalized() {
        "python" => Ok("cpython"),
        "pypy" => Ok("pypy"),
        _ => Err(UnsupportedPythonImplementationError(
            python_record.name.as_normalized().to_string(),
        )),
    }
}

/// Return whether the specified record has gil disabled (by being a free-threaded python interpreter)
fn gil_disabled(python_record: &PackageRecord) -> miette::Result<bool> {
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
        .collect::<Result<Vec<MatchSpec>, _>>()
        .into_diagnostic()?;

    Ok(deps.iter().any(|spec| {
        spec.name
            .as_ref()
            .is_some_and(|name| name.as_source() == "python_abi")
            && spec.build.as_ref().is_some_and(|build| {
                let raw_str = format!("{}", build);
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
) -> miette::Result<Tags> {
    // Build the wheel tags based on the interpreter, the target platform, and the python version.
    let tags = Tags::from_env(
        &platform,
        python_version,
        implementation_name,
        // TODO: This might not be entirely correct..
        python_version,
        true,
        gil_disabled,
    )
    .into_diagnostic()
    .context("failed to determine the python wheel tags for the target platform")?;

    Ok(tags)
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformTagError {
    #[error("Unsupported platform {0} for pypi tags")]
    UnsupportedVirtualPackage(String),
    #[error("Version {0} to high to cast down for platform tag creation")]
    VersionCastError(u64),
    #[error("Expected virtual package {0} for {1}.")]
    ExpectedVirtualPackage(String, String),
    #[error("Expected virtual package with version for: {0} but got: {1}")]
    ExpectedVirtualPackageWithVersion(String, String),
    #[error("Can't find a pypi tags for the platform: {0}, this is not your fault, please report this issue.")]
    NoTagsForPlatform(String),
    #[error(transparent)]
    ArchTagsError(#[from] ArchTagsError),
}

/// Get the pypi platform from the conda virtual packages
/// Used to get the platform for the environment validation in the lock file.
fn get_pypi_platform_from_virtual_packages(
    virtual_packages: &[VirtualPackage],
    platform: Platform,
) -> Result<uv_platform_tags::Platform, PlatformTagError> {
    if platform.is_linux() {
        // The linux platform is mostly based on the libc version
        let libc = virtual_packages
            .iter()
            .find_map(|package| match package {
                VirtualPackage::LibC(libc) => Some(libc),
                _ => None,
            })
            .ok_or(PlatformTagError::ExpectedVirtualPackage(
                "libc".to_string(),
                platform.to_string(),
            ))?;

        let (major, minor) = libc.version.as_major_minor().ok_or(
            PlatformTagError::ExpectedVirtualPackageWithVersion(
                platform.to_string(),
                libc.version.to_string(),
            ),
        )?;
        // Protect casting with an error to avoid hard to find bugs
        let major = u64::try_into(major).map_err(|_| PlatformTagError::VersionCastError(major))?;
        let minor = u64::try_into(minor).map_err(|_| PlatformTagError::VersionCastError(minor))?;

        return match libc.family.to_lowercase().as_str() {
            "glibc" => Ok(uv_platform_tags::Platform::new(
                UvOs::Manylinux { major, minor },
                get_arch_tags(platform)?,
            )),
            "musl" => Ok(uv_platform_tags::Platform::new(
                UvOs::Musllinux { major, minor },
                get_arch_tags(platform)?,
            )),
            // TODO: Add more libc families for support of other linux distributions
            _ => Err(PlatformTagError::UnsupportedVirtualPackage(
                libc.to_string(),
            )),
        };
    }

    if platform.is_windows() {
        return Ok(uv_platform_tags::Platform::new(
            UvOs::Windows,
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
            .ok_or(PlatformTagError::ExpectedVirtualPackage(
                "osx".to_string(),
                platform.to_string(),
            ))?;

        let (major, minor) = osx.version.as_major_minor().ok_or(
            PlatformTagError::ExpectedVirtualPackageWithVersion(
                platform.to_string(),
                osx.version.to_string(),
            ),
        )?;
        // Protect casting with an error to avoid hard to find bugs
        let major = u64::try_into(major).map_err(|_| PlatformTagError::VersionCastError(major))?;
        let minor = u64::try_into(minor).map_err(|_| PlatformTagError::VersionCastError(minor))?;

        return Ok(uv_platform_tags::Platform::new(
            UvOs::Macos { major, minor },
            get_arch_tags(platform.to_owned())?,
        ));
    }

    Err(PlatformTagError::NoTagsForPlatform(platform.to_string()))
}

/// Get the pypi tags for this machine and the given python record
/// Designed to work for the environment validation in the lock file with the current machine.
pub fn get_tags_from_machine(
    virtual_packages: &[VirtualPackage],
    platform: Platform,
    python_record: &PackageRecord,
) -> miette::Result<Tags> {
    let platform =
        get_pypi_platform_from_virtual_packages(virtual_packages, platform).into_diagnostic()?;
    create_tags(
        platform,
        get_python_version(python_record).into_diagnostic()?,
        get_implementation_name(python_record).into_diagnostic()?,
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
            &UvOs::Macos {
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
            &UvOs::Macos {
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
            &UvOs::Manylinux {
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
        assert_eq!(platform.os(), &UvOs::Musllinux { major: 1, minor: 2 });
        assert_eq!(platform.arch(), UvArch::X86_64);

        let platform = Platform::LinuxAarch64;
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
        let platform = res.unwrap();
        assert_eq!(platform.os(), &UvOs::Musllinux { major: 1, minor: 2 });
        assert_eq!(platform.arch(), UvArch::Aarch64);

        let vpkgs = vec![VirtualPackage::LibC(LibC {
            family: "musl".to_string(),
            version: "1.2".parse().unwrap(),
        })];
        let platform = Platform::LinuxPpc64le;
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
        let platform = res.unwrap();
        assert_eq!(platform.os(), &UvOs::Musllinux { major: 1, minor: 2 });
        assert_eq!(platform.arch(), UvArch::Powerpc64Le);
    }

    #[test]
    fn test_get_platform_from_vpkgs_windows() {
        let vpkgs = vec![];
        let platform = Platform::Win64;
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
        let platform = res.unwrap();
        assert_eq!(platform.os(), &UvOs::Windows);
        assert_eq!(platform.arch(), UvArch::X86_64);

        let platform = Platform::WinArm64;
        let res = get_pypi_platform_from_virtual_packages(&vpkgs, platform);
        let platform = res.unwrap();
        assert_eq!(platform.os(), &UvOs::Windows);
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
            PlatformTagError::UnsupportedVirtualPackage(_)
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
