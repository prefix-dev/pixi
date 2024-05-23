use pep508_rs::{MarkerEnvironment, StringVersion};
use rattler_conda_types::{PackageRecord, Platform, VersionWithSource};
use std::str::FromStr;

/// Determine the available env markers based on the platform and python package.
pub fn determine_marker_environment(
    platform: Platform,
    python_record: &PackageRecord,
) -> miette::Result<MarkerEnvironment> {
    // Determine system specific information
    let (sys_platform, platform_system, os_name) = if platform.is_linux() {
        ("linux", "Linux", "posix")
    } else if platform.is_osx() {
        ("darwin", "Darwin", "posix")
    } else if platform.is_windows() {
        ("win32", "Windows", "nt")
    } else {
        miette::bail!("could not determine python environment markers for {platform}")
    };

    // Determine implementation name
    let (implementation_name, platform_python_implementation) =
        if python_record.name.as_normalized() == "python" {
            ("cpython", "CPython")
        } else {
            miette::bail!(
                "unsupported python variant {}",
                python_record.name.as_source()
            )
        };

    let platform_machine = match platform {
        Platform::Linux32 => "i386",
        Platform::Linux64 => "x86_64",
        Platform::LinuxAarch64 => "aarch64",
        Platform::LinuxArmV6l => "armv6l",
        Platform::LinuxArmV7l => "armv7l",
        Platform::LinuxPpc64le => "ppc64le",
        Platform::LinuxPpc64 => "ppc64",
        Platform::LinuxS390X => "s390x",
        Platform::LinuxRiscv32 => "riscv32",
        Platform::LinuxRiscv64 => "riscv64",
        Platform::Osx64 => "x86_64",
        Platform::OsxArm64 => "arm64",
        Platform::Win32 => "x86",
        Platform::Win64 => "AMD64",
        Platform::WinArm64 => "ARM64",
        _ => "",
    };

    MarkerEnvironment::new(MarkerEnvironment {
        implementation_name: String::from(implementation_name),
        implementation_version: version_to_string_version(&python_record.version),
        os_name: String::from(os_name),
        platform_python_implementation: String::from(platform_python_implementation),
        platform_system: String::from(platform_system),
        python_full_version: version_to_string_version(&python_record.version),
        python_version: python_record
            .version
            .version()
            .as_major_minor()
            .and_then(|(major, minor)| StringVersion::from_str(&format!("{major}.{minor}")).ok())
            .ok_or_else(|| {
                miette::miette!(
                    "could not convert python version {}, to a major minor version",
                    &python_record.version
                )
            })?,
        sys_platform: String::from(sys_platform),
        platform_machine: String::from(platform_machine),

        // I assume we can leave these empty
        platform_release: "".to_string(),
        platform_version: "".to_string(),
    })
}

/// Convert a [`VersionWithSource`] to a [`StringVersion`].
fn version_to_string_version(version: &VersionWithSource) -> StringVersion {
    StringVersion::from_str(&version.to_string()).expect("could not convert between versions")
}
