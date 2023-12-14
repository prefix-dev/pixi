use crate::{
    consts::PROJECT_MANIFEST,
    lock_file::{package_identifier, pypi_name_mapping},
    project::{manifest::LibCSystemRequirement, manifest::SystemRequirements},
    virtual_packages::{default_glibc_version, default_mac_os_version},
    Project,
};
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pep508_rs::{MarkerEnvironment, StringVersion};
use rattler_conda_types::{PackageRecord, Platform, RepoDataRecord, Version, VersionWithSource};
use rip::python_env::{WheelTag, WheelTags};
use rip::resolve::{resolve, PinnedPackage, ResolveOptions, SDistResolution};
use std::{collections::HashMap, str::FromStr, vec};

/// Resolve python packages for the specified project.
pub async fn resolve_pypi_dependencies<'p>(
    project: &'p Project,
    platform: Platform,
    conda_packages: &mut [RepoDataRecord],
) -> miette::Result<Vec<PinnedPackage<'p>>> {
    let dependencies = project.pypi_dependencies(platform);
    if dependencies.is_empty() {
        return Ok(vec![]);
    }

    // Amend the records with pypi purls if they are not present yet.
    let conda_forge_mapping = pypi_name_mapping::conda_pypi_name_mapping().await?;
    for record in conda_packages.iter_mut() {
        pypi_name_mapping::amend_pypi_purls(record, conda_forge_mapping)?;
    }

    // Determine the python packages that are installed by the conda packages
    let conda_python_packages =
        package_identifier::PypiPackageIdentifier::from_records(conda_packages)
            .into_diagnostic()
            .context("failed to extract python packages from conda metadata")?
            .into_iter()
            .map(PinnedPackage::from)
            .collect_vec();

    if !conda_python_packages.is_empty() {
        tracing::info!(
            "the following python packages are assumed to be installed by conda: {conda_python_packages}",
            conda_python_packages =
                conda_python_packages
                    .iter()
                    .format_with(", ", |p, f| f(&format_args!(
                        "{name} {version}",
                        name = &p.name,
                        version = &p.version
                    )))
        );
    } else {
        tracing::info!("there are no python packages installed by conda");
    }

    // Determine the python interpreter that is installed as part of the conda packages.
    let python_record = conda_packages
        .iter()
        .find(|r| is_python_record(r))
        .ok_or_else(|| miette::miette!("could not resolve pypi dependencies because no python interpreter is added to the dependencies of the project.\nMake sure to add a python interpreter to the [dependencies] section of the {PROJECT_MANIFEST}, or run:\n\n\tpixi add python"))?;

    // Determine the environment markers
    let marker_environment = determine_marker_environment(platform, python_record.as_ref())?;

    // Determine the compatible tags
    let compatible_tags = project_platform_tags(
        platform,
        &project.manifest.system_requirements,
        python_record.as_ref(),
    );

    let requirements = dependencies
        .iter()
        .map(|(name, req)| req.as_pep508(name))
        .collect::<Vec<pep508_rs::Requirement>>();

    // Resolve the PyPi dependencies
    let mut result = resolve(
        project.pypi_package_db()?,
        &requirements,
        &marker_environment,
        Some(&compatible_tags),
        conda_python_packages
            .into_iter()
            .map(|p| (p.name.clone(), p))
            .collect(),
        HashMap::default(),
        &ResolveOptions {
            // TODO: Change this once we fully support sdists.
            sdist_resolution: SDistResolution::OnlyWheels,
        },
    )
    .await?;

    // Remove any conda package from the result
    result.retain(|p| !p.artifacts.is_empty());

    Ok(result)
}

/// Returns true if the specified record refers to a version/variant of python.
pub fn is_python_record(record: &RepoDataRecord) -> bool {
    package_name_is_python(&record.package_record.name)
}

/// Returns true if the specified name refers to a version/variant of python.
/// TODO: Add support for more variants.
pub fn package_name_is_python(record: &rattler_conda_types::PackageName) -> bool {
    record.as_normalized() == "python"
}

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

    Ok(MarkerEnvironment {
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

/// Returns the compatible tags for the project on the given platform with the given python package.
fn project_platform_tags(
    platform: Platform,
    system_requirements: &SystemRequirements,
    python_record: &PackageRecord,
) -> WheelTags {
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

fn mac_platform_tags(platform: Platform, mac_version: &Version) -> Vec<String> {
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
fn linux_platform_tags(platform: Platform, max_glibc_version: &Version) -> Vec<String> {
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
fn cpython_tags<P, PIter>(python_version: &Version, platforms: PIter) -> Vec<WheelTag>
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
fn py_interpreter_range(python_version: &Version) -> impl Iterator<Item = String> {
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
fn compatible_tags<'a, P, PIter>(
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
