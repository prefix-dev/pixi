use miette::{Context, IntoDiagnostic};
use pixi_default_versions::{default_glibc_version, default_mac_os_version};
use pixi_manifest::{LibCSystemRequirement, SystemRequirements};
use platform_tags::Os;
use platform_tags::Tags;
use rattler_conda_types::{Arch, PackageRecord, Platform};

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
    let python_version = get_python_version(python_record)?;
    let implementation_name = get_implementation_name(python_record)?;
    create_tags(platform, python_version, implementation_name)
}

/// Create a uv platform tag for the specified platform
fn get_platform_tags(
    platform: Platform,
    system_requirements: &SystemRequirements,
) -> miette::Result<platform_tags::Platform> {
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
) -> miette::Result<platform_tags::Platform> {
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
            Ok(platform_tags::Platform::new(
                Os::Manylinux {
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
            Ok(platform_tags::Platform::new(
                Os::Manylinux {
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
fn get_windows_platform_tags(platform: Platform) -> miette::Result<platform_tags::Platform> {
    let arch = get_arch_tags(platform)?;
    Ok(platform_tags::Platform::new(Os::Windows, arch))
}

/// Get macos specific platform tags
fn get_macos_platform_tags(
    platform: Platform,
    system_requirements: &SystemRequirements,
) -> miette::Result<platform_tags::Platform> {
    let osx_version = system_requirements
        .macos
        .clone()
        .unwrap_or_else(|| default_mac_os_version(platform));
    let Some((major, minor)) = osx_version.as_major_minor() else {
        miette::bail!("expected macos version to be a major.minor version, but got '{osx_version}'")
    };

    let arch = get_arch_tags(platform)?;

    Ok(platform_tags::Platform::new(
        Os::Macos {
            major: major as _,
            minor: minor as _,
        },
        arch,
    ))
}

/// Get the arch tag for the specified platform
fn get_arch_tags(platform: Platform) -> miette::Result<platform_tags::Arch> {
    match platform.arch() {
        None => unreachable!("every platform we support has an arch"),
        Some(Arch::X86) => Ok(platform_tags::Arch::X86),
        Some(Arch::X86_64) => Ok(platform_tags::Arch::X86_64),
        Some(Arch::Aarch64 | Arch::Arm64) => Ok(platform_tags::Arch::Aarch64),
        Some(Arch::ArmV7l) => Ok(platform_tags::Arch::Armv7L),
        Some(Arch::Ppc64le) => Ok(platform_tags::Arch::Powerpc64Le),
        Some(Arch::Ppc64) => Ok(platform_tags::Arch::Powerpc64),
        Some(Arch::S390X) => Ok(platform_tags::Arch::S390X),
        Some(unsupported_arch) => {
            miette::bail!("unsupported arch for pypi packages '{unsupported_arch}'")
        }
    }
}

fn get_python_version(python_record: &PackageRecord) -> miette::Result<(u8, u8)> {
    let Some(python_version) = python_record.version.as_major_minor() else {
        miette::bail!(
            "expected python version to be a major.minor version, but got '{}'",
            &python_record.version
        );
    };
    Ok((python_version.0 as u8, python_version.1 as u8))
}

fn get_implementation_name(python_record: &PackageRecord) -> miette::Result<&'static str> {
    match python_record.name.as_normalized() {
        "python" => Ok("cpython"),
        "pypy" => Ok("pypy"),
        _ => {
            miette::bail!(
                "unsupported python implementation '{}'",
                python_record.name.as_source()
            );
        }
    }
}

fn create_tags(
    platform: platform_tags::Platform,
    python_version: (u8, u8),
    implementation_name: &str,
) -> miette::Result<Tags> {
    // Build the wheel tags based on the interpreter, the target platform, and the python version.
    let tags = Tags::from_env(
        &platform,
        python_version,
        implementation_name,
        // TODO: This might not be entirely correct..
        python_version,
        true,
        // Should revisit this when this lands: https://github.com/conda-forge/python-feedstock/pull/679
        false,
    )
    .into_diagnostic()
    .context("failed to determine the python wheel tags for the target platform")?;

    Ok(tags)
}
