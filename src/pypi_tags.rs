use crate::project::virtual_packages::{default_glibc_version, default_mac_os_version};
use miette::{Context, IntoDiagnostic};
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

pub fn get_pypi_tags(
    platform: Platform,
    system_requirements: &SystemRequirements,
    python_record: &PackageRecord,
) -> miette::Result<Tags> {
    let platform = if platform.is_linux() {
        let arch = match platform.arch() {
            None => unreachable!("every platform we support has an arch"),
            Some(Arch::X86) => platform_tags::Arch::X86,
            Some(Arch::X86_64) => platform_tags::Arch::X86_64,
            Some(Arch::Aarch64 | Arch::Arm64) => platform_tags::Arch::Aarch64,
            Some(Arch::ArmV7l) => platform_tags::Arch::Armv7L,
            Some(Arch::Ppc64le) => platform_tags::Arch::Powerpc64Le,
            Some(Arch::Ppc64) => platform_tags::Arch::Powerpc64,
            Some(Arch::S390X) => platform_tags::Arch::S390X,
            Some(unsupported_arch) => {
                miette::bail!("unsupported arch for pypi packages '{unsupported_arch}'")
            }
        };

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
                platform_tags::Platform::new(
                    Os::Manylinux {
                        major: major as _,
                        minor: minor as _,
                    },
                    arch,
                )
            }
            Some(("glibc", version)) => {
                let Some((major, minor)) = version.as_major_minor() else {
                    miette::bail!(
                        "expected glibc version to be a major.minor version, but got '{version}'"
                    )
                };
                platform_tags::Platform::new(
                    Os::Manylinux {
                        major: major as _,
                        minor: minor as _,
                    },
                    arch,
                )
            }
            Some((family, _)) => {
                miette::bail!("unsupported libc family for pypi packages '{family}'");
            }
        }
    } else if platform.is_windows() {
        let arch = match platform.arch() {
            None => unreachable!("every platform we support has an arch"),
            Some(Arch::X86) => platform_tags::Arch::X86,
            Some(Arch::X86_64) => platform_tags::Arch::X86_64,
            Some(Arch::Aarch64 | Arch::Arm64) => platform_tags::Arch::Aarch64,
            Some(unsupported_arch) => {
                miette::bail!("unsupported arch for pypi packages '{unsupported_arch}'")
            }
        };

        platform_tags::Platform::new(Os::Windows, arch)
    } else if platform.is_osx() {
        let osx_version = system_requirements
            .macos
            .clone()
            .unwrap_or_else(|| default_mac_os_version(platform));
        let Some((major, minor)) = osx_version.as_major_minor() else {
            miette::bail!(
                "expected macos version to be a major.minor version, but got '{osx_version}'"
            )
        };

        let arch = match platform.arch() {
            None => unreachable!("every platform we support has an arch"),
            Some(Arch::X86) => platform_tags::Arch::X86,
            Some(Arch::X86_64) => platform_tags::Arch::X86_64,
            Some(Arch::Aarch64 | Arch::Arm64) => platform_tags::Arch::Aarch64,
            Some(unsupported_arch) => {
                miette::bail!("unsupported arch for pypi packages '{unsupported_arch}'")
            }
        };

        platform_tags::Platform::new(
            Os::Macos {
                major: major as _,
                minor: minor as _,
            },
            arch,
        )
    } else {
        miette::bail!("unsupported platform for pypi packages {platform}")
    };

    // Build the wheel tags based on the interpreter, the target platform, and the python version.
    let Some(python_version) = python_record.version.as_major_minor() else {
        miette::bail!(
            "expected python version to be a major.minor version, but got '{}'",
            &python_record.version
        );
    };
    let implementation_name = match python_record.name.as_normalized() {
        "python" => "cpython",
        "pypy" => "pypy",
        _ => {
            miette::bail!(
                "unsupported python implementation '{}'",
                python_record.name.as_source()
            );
        }
    };
    let tags = Tags::from_env(
        &platform,
        (python_version.0 as u8, python_version.1 as u8),
        implementation_name,
        // TODO: This might not be entirely correct..
        (python_version.0 as u8, python_version.1 as u8),
        false,
    )
    .into_diagnostic()
    .context("failed to determine the python wheel tags for the target platform")?;

    Ok(tags)
}
