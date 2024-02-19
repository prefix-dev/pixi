use crate::project::manifest::{LibCSystemRequirement, SystemRequirements};
use crate::project::virtual_packages::{default_glibc_version, default_mac_os_version};
use itertools::Itertools;
use rattler_conda_types::{PackageRecord, Platform, Version};
use rip::python_env::{WheelTag, WheelTags};
use std::str::FromStr;

/// Returns true if the specified record refers to a version/variant of python.
pub fn is_python_record(record: impl AsRef<PackageRecord>) -> bool {
    package_name_is_python(&record.as_ref().name)
}

/// Returns true if the specified name refers to a version/variant of python.
/// TODO: Add support for more variants.
pub fn package_name_is_python(record: &rattler_conda_types::PackageName) -> bool {
    record.as_normalized() == "python"
}

/// Returns the compatible tags for the project on the given platform with the given python package.
pub fn project_platform_tags(
    platform: Platform,
    system_requirements: &SystemRequirements,
    python_record: &PackageRecord,
) -> Tags {
    let platforms = project_platforms(platform, system_requirements);

    let mut tags = Vec::new();

    if python_record.name.as_normalized() == "python" {
        tags.append(&mut cpython_tags(&python_record.version, &platforms));
    } else {
        todo!("no support for generic tags yet");
    }

    tags.append(&mut compatible_tags(&python_record.version, &platforms).collect());

    WheelTags::from_iter(tags)
}

fn project_platforms(platform: Platform, system_requirements: &SystemRequirements) -> Vec<String> {
    if platform.is_windows() {
        match platform {
            Platform::Win32 => vec![String::from("win32")],
            Platform::Win64 => vec![String::from("win_amd64")],
            Platform::WinArm64 => vec![String::from("win_arm64")],
            _ => unreachable!("not windows"),
        }
    } else if platform.is_linux() {
        let max_glibc_version = match system_requirements
            .libc
            .as_ref()
            .map(LibCSystemRequirement::family_and_version)
        {
            Some((family, version)) if family.eq_ignore_ascii_case("glibc") => version.clone(),
            Some(_) => {
                // Another libc family is being target.
                return Vec::new();
            }
            None => default_glibc_version(),
        };
        linux_platform_tags(platform, &max_glibc_version)
    } else {
        let mac_version = system_requirements
            .macos
            .as_ref()
            .map_or_else(|| default_mac_os_version(platform), |v| v.clone());
        mac_platform_tags(platform, &mac_version)
    }
}

pub fn mac_platform_tags(platform: Platform, mac_version: &Version) -> Vec<String> {
    let v10_0 = Version::from_str("10.0").unwrap();
    let v11_0 = Version::from_str("11.0").unwrap();
    let (major, minor) = mac_version.as_major_minor().expect("invalid mac version");

    let mut result = Vec::new();

    // Prior to macOS 11, each yearly release of macOS bumped the "minor" version number. The
    // major version was always 10.
    if mac_version >= &v10_0 && mac_version < &v11_0 {
        let binary_formats = mac_binary_formats(mac_version, platform);
        for (minor, binary_format) in (0..=minor).rev().cartesian_product(binary_formats.iter()) {
            result.push(format!("macosx_{major}_{minor}_{binary_format}"));
        }
    }

    // Starting with macOS 11, each yearly release bumps the major version number. The minor
    // versions are no the midyear updates.
    if mac_version >= &v11_0 {
        let binary_formats = mac_binary_formats(mac_version, platform);
        for (major, binary_format) in (11..=major).rev().cartesian_product(binary_formats.iter()) {
            result.push(format!("macosx_{major}_{minor}_{binary_format}", minor = 0));
        }
    }

    // macOS 11 on x86_64 is compatible with binaries from previous releases.
    // Arm64 support was introduced in 11.0, so no Arm binaries from previous releases exist.
    //
    // However, the "universal2" binary format can have a macOS version earlier than 11.0 when the
    // x86_64 part of the binary supports that version of macOS.
    if mac_version >= &v11_0 {
        for minor in (4..=16).rev() {
            let binary_formats = if platform == Platform::Osx64 {
                let compatible_version = Version::from_str(&format!("10.{}", minor)).unwrap();
                mac_binary_formats(&compatible_version, platform)
            } else {
                vec![String::from("universal2")]
            };
            for binary_format in binary_formats {
                result.push(format!(
                    "macosx_{major}_{minor}_{binary_format}",
                    major = 10,
                    minor = minor,
                    binary_format = binary_format
                ));
            }
        }
    }

    result
}

/// Returns a list of compatible binary formats for the specified mac version and platform.
fn mac_binary_formats(mac_version: &Version, platform: Platform) -> Vec<String> {
    let mut result = match platform {
        Platform::Osx64 => vec![String::from("x86_64")],
        Platform::OsxArm64 => vec![String::from("arm64")],
        _ => unreachable!("unsupported mac platform: {platform}"),
    };
    let v10_4 = Version::from_str("10.4").unwrap();

    if platform == Platform::Osx64 && mac_version >= &v10_4 {
        result.extend([
            String::from("intel"),
            String::from("fat64"),
            String::from("fat32"),
        ]);
    }

    if matches!(platform, Platform::Osx64 | Platform::OsxArm64) {
        result.push(String::from("universal2"));
    }

    if matches!(platform, Platform::Osx64) {
        result.push(String::from("universal"));
    }

    result
}

/// Returns the platform tags for linux based OS
pub fn linux_platform_tags(platform: Platform, max_glibc_version: &Version) -> Vec<String> {
    let arch = match platform {
        Platform::Linux32 => "_i686",
        Platform::Linux64 => "_x86_64",
        Platform::LinuxAarch64 => "_aarch64",
        Platform::LinuxArmV7l => "_armv7l",
        Platform::LinuxPpc64le => "_ppc64le",
        Platform::LinuxPpc64 => "_ppc64",
        Platform::LinuxS390X => "_s390x",
        Platform::LinuxRiscv32 => return Vec::new(),
        Platform::LinuxRiscv64 => return Vec::new(),
        Platform::LinuxArmV6l => return Vec::new(),
        _ => unreachable!("not linux"),
    };
    manylinux_versions(max_glibc_version)
        .into_iter()
        .map(|p| format!("{p}{arch}"))
        .collect()
}

// Generate all manylinux tags based on the major minor version of glibc.
fn manylinux_versions(glibc_version: &Version) -> Vec<String> {
    let Some((major, minor)) = glibc_version.as_major_minor() else {
        return Vec::new();
    };

    let mut result = Vec::new();
    for minor in (0..=minor).rev() {
        result.push(format!("manylinux_{major}_{minor}"));
        match (major, minor) {
            (2, 5) => result.push(String::from("manylinux1")),
            (2, 12) => result.push(String::from("manylinux2010")),
            (2, 17) => result.push(String::from("manylinux2014")),
            _ => {}
        }
    }

    result
}

/// Returns all the tags a specific cpython implementation supports.
pub fn cpython_tags<P, PIter>(python_version: &Version, platforms: PIter) -> Vec<WheelTag>
where
    P: Into<String>,
    PIter: IntoIterator<Item = P>,
    PIter::IntoIter: Clone,
{
    let Some((major, minor)) = python_version.as_major_minor() else {
        return Vec::new();
    };

    let interpreter = format!("cp{major}{minor}");
    let core_abi = format!("cp{}{}", major, minor);

    let mut result = Vec::new();
    let platforms = platforms.into_iter().map(Into::into);

    // Add the core tags
    result.extend(platforms.clone().map(|platform| WheelTag {
        interpreter: interpreter.clone(),
        abi: core_abi.clone(),
        platform,
    }));

    // Add the "abi3"  tags
    if python_abi3_applies(major, minor) {
        result.extend(platforms.clone().map(|platform| WheelTag {
            interpreter: interpreter.clone(),
            abi: String::from("abi3"),
            platform,
        }));
    }

    // Add the "none" abi tags
    result.extend(platforms.clone().map(|platform| WheelTag {
        interpreter: interpreter.clone(),
        abi: String::from("none"),
        platform,
    }));

    // Add other abi3 compatible cpython versions
    if python_abi3_applies(major, minor) {
        for minor in (2..minor).rev() {
            let interpreter = format!("cp{major}{minor}");
            result.extend(platforms.clone().map(|platform| WheelTag {
                interpreter: interpreter.clone(),
                abi: String::from("abi3"),
                platform,
            }));
        }
    }

    result
}

/// Returns true if the specified Python version supports abi3. PEP 384 was first implemented in
/// Python 3.2.
fn python_abi3_applies(major: u64, minor: u64) -> bool {
    major >= 3 && minor >= 2
}

/// Returns an iterator that yields all the different versions of the python interpreter.
pub fn py_interpreter_range(python_version: &Version) -> impl Iterator<Item = String> {
    let python_version = python_version.as_major_minor();

    let core_version = python_version
        .into_iter()
        .map(|(major, minor)| format!("py{major}{minor}"));
    let major_version = python_version
        .into_iter()
        .map(|(major, _)| format!("py{major}"));
    let minor_versions = python_version.into_iter().flat_map(|(major, minor)| {
        (0..minor)
            .rev()
            .map(move |minor| format!("py{major}{minor}"))
    });
    core_version.chain(major_version).chain(minor_versions)
}

/// Returns compatible tag for any interpreter.
pub fn compatible_tags<'a, P, PIter>(
    python_version: &'a Version,
    platforms: PIter,
) -> impl Iterator<Item = WheelTag> + 'a
where
    P: Into<String>,
    PIter: IntoIterator<Item = P>,
    PIter::IntoIter: Clone + 'a,
{
    py_interpreter_range(python_version)
        .cartesian_product(platforms)
        .map(|(interpreter, platform)| WheelTag {
            interpreter,
            abi: String::from("none"),
            platform: platform.into(),
        })
        .chain(
            py_interpreter_range(python_version).map(|interpreter| WheelTag {
                interpreter,
                abi: String::from("none"),
                platform: String::from("any"),
            }),
        )
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_cpython_tags() {
        let tags: Vec<_> = cpython_tags(&Version::from_str("3.11.2").unwrap(), vec!["win_amd64"])
            .into_iter()
            .map(|t| t.to_string())
            .collect();
        insta::assert_debug_snapshot!(tags);
    }

    #[test]
    fn test_py_interpreter_range() {
        let tags: Vec<_> = py_interpreter_range(&Version::from_str("3.11.2").unwrap()).collect();
        insta::assert_debug_snapshot!(tags);
    }

    #[test]
    fn test_compatible_tags() {
        let tags: Vec<_> =
            compatible_tags(&Version::from_str("3.11.2").unwrap(), vec!["win_amd64"])
                .map(|t| t.to_string())
                .collect();
        insta::assert_debug_snapshot!(tags);
    }

    #[test]
    fn test_linux_platform_tags() {
        let tags: Vec<_> =
            linux_platform_tags(Platform::Linux64, &Version::from_str("2.17").unwrap())
                .into_iter()
                .map(|t| t.to_string())
                .collect();
        insta::assert_debug_snapshot!(tags);
    }

    #[test]
    fn test_mac_platform_tags() {
        let tags: Vec<_> =
            mac_platform_tags(Platform::OsxArm64, &Version::from_str("14.0").unwrap())
                .into_iter()
                .map(|t| t.to_string())
                .collect();
        insta::assert_debug_snapshot!(tags);
    }
}
