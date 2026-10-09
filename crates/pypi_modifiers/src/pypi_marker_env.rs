use pixi_manifest::PixiPlatform;
use rattler_conda_types::{PackageRecord, Subdir};
use uv_pep508::{MarkerEnvironment, MarkerEnvironmentBuilder, uv_pep440::VersionParseError};

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum MarkerEnvironmentError {
    #[error("could not determine python environment markers for {0}")]
    UnsupportedPlatform(String),

    #[error("unsupported python variant {0}")]
    UnsupportedPythonVariant(String),

    #[error("could not convert python version {0}, to a major minor version")]
    NoMajorMinorVersion(String),

    // Inline rather than `#[source]`: callers log this with `{e}`.
    #[error("{0}")]
    InvalidVersion(VersionParseError),
}

/// Determine the available env markers based on the platform and python package.
pub fn determine_marker_environment(
    platform: &PixiPlatform,
    python_record: &PackageRecord,
) -> Result<MarkerEnvironment, MarkerEnvironmentError> {
    let subdir = platform.subdir();
    // Determine system specific information
    let (sys_platform, platform_system, os_name) = if subdir.is_linux() {
        ("linux", "Linux", "posix")
    } else if subdir.is_osx() {
        ("darwin", "Darwin", "posix")
    } else if subdir.is_windows() {
        ("win32", "Windows", "nt")
    } else {
        return Err(MarkerEnvironmentError::UnsupportedPlatform(
            platform.to_string(),
        ));
    };

    // Determine implementation name
    let (implementation_name, platform_python_implementation) =
        if python_record.name.as_normalized() == "python" {
            ("cpython", "CPython")
        } else {
            return Err(MarkerEnvironmentError::UnsupportedPythonVariant(
                python_record.name.as_source().to_string(),
            ));
        };

    let platform_machine = match subdir {
        Subdir::Linux32 => "i386",
        Subdir::Linux64 => "x86_64",
        Subdir::LinuxAarch64 => "aarch64",
        Subdir::LinuxArmV6l => "armv6l",
        Subdir::LinuxArmV7l => "armv7l",
        Subdir::LinuxPpc64le => "ppc64le",
        Subdir::LinuxPpc64 => "ppc64",
        Subdir::LinuxS390X => "s390x",
        Subdir::LinuxRiscv32 => "riscv32",
        Subdir::LinuxRiscv64 => "riscv64",
        Subdir::Osx64 => "x86_64",
        Subdir::OsxArm64 => "arm64",
        Subdir::Win32 => "x86",
        Subdir::Win64 => "AMD64",
        Subdir::WinArm64 => "ARM64",
        _ => "",
    };

    MarkerEnvironment::try_from(MarkerEnvironmentBuilder {
        implementation_name,
        implementation_version: python_record.version.as_str().as_ref(),
        os_name,
        platform_python_implementation,
        platform_system,
        python_full_version: python_record.version.as_str().as_ref(),
        python_version: &python_record
            .version
            .version()
            .as_major_minor()
            .map(|(major, minor)| format!("{major}.{minor}"))
            .ok_or_else(|| {
                MarkerEnvironmentError::NoMajorMinorVersion(python_record.version.to_string())
            })?,
        sys_platform,
        platform_machine,

        // I assume we can leave these empty
        platform_release: "",
        platform_version: "",
    })
    .map_err(MarkerEnvironmentError::InvalidVersion)
}
