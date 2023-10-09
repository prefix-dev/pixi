use crate::consts::PROJECT_MANIFEST;
use crate::project::manifest::LibCSystemRequirement;
use crate::virtual_packages::default_glibc_version;
use crate::Project;
use itertools::Itertools;
use pep508_rs::{MarkerEnvironment, StringVersion};
use rattler_conda_types::{PackageRecord, Platform, RepoDataRecord, Version, VersionWithSource};
use rip::tags::{WheelTag, WheelTags};
use rip::PinnedPackage;
use std::str::FromStr;
use std::vec;

/// Resolve python packages for the specified project.
pub async fn resolve_python_dependencies<'p>(
    project: &'p Project,
    platform: Platform,
    conda_packages: &[RepoDataRecord],
) -> miette::Result<Vec<PinnedPackage<'p>>> {
    let requirements = project.python_dependencies();
    if requirements.is_empty() {
        // If there are no requirements we can skip this function.
        return Ok(vec![]);
    }

    // Determine the python interpreter that is installed as part of the conda packages.
    let python_record = conda_packages
        .iter()
        .find(|r| is_python(r))
        .ok_or_else(|| miette::miette!("could not resolve python dependencies because no python interpreter is added to the dependencies of the project.\nMake sure to add a python interpreter to the [dependencies] section of the {PROJECT_MANIFEST}, e.g.:.\n\n\t[dependencies]\n\tpython = \"*\""))?;

    // Determine the environment markers
    let marker_environment = determine_marker_environment(platform, python_record.as_ref())?;

    // Determine the compatible tags
    let compatible_tags = project_platform_tags(project, platform, python_record.as_ref());

    // Resolve the PyPi dependencies
    let result = rip::resolve(
        project.python_package_db()?,
        &requirements.as_pep508(),
        &marker_environment,
        Some(&compatible_tags),
    )
    .await?;

    Ok(result)
}

/// Returns true if the specified record refers to a version/variant of python.
/// TODO: Add support for more variants.
pub fn is_python(record: &RepoDataRecord) -> bool {
    record.package_record.name.as_normalized() == "python"
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

        // TODO: Can we figure this out?
        platform_machine: "".to_string(),

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
    project: &Project,
    platform: Platform,
    python_record: &PackageRecord,
) -> WheelTags {
    let platforms = project_platforms(project, platform);

    let mut tags = Vec::new();

    if python_record.name.as_normalized() == "python" {
        tags.append(&mut cpython_tags(&python_record.version, &platforms));
    } else {
        todo!("no support for generic tags yet");
    }

    tags.append(&mut compatible_tags(&python_record.version, &platforms).collect());

    WheelTags::from_iter(tags.into_iter())
}

fn project_platforms(project: &Project, platform: Platform) -> Vec<String> {
    if platform.is_windows() {
        match platform {
            Platform::Win32 => vec![String::from("win32")],
            Platform::Win64 => vec![String::from("win_amd64")],
            Platform::WinArm64 => vec![String::from("win_arm64")],
            _ => unreachable!("not windows"),
        }
    } else if platform.is_linux() {
        let max_glibc_version = match &project.manifest.system_requirements.libc {
            Some(LibCSystemRequirement::GlibC(v)) => v.clone(),
            Some(LibCSystemRequirement::OtherFamily(_)) => return Vec::new(),
            None => default_glibc_version(),
        };
        linux_platform_tags(platform, &max_glibc_version)
    } else {
        todo!("no implementation for mac yet!")
    }
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
    let Some((major, minor)) = glibc_version.as_major_minor() else { return Vec::new() };

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
    let Some((major, minor)) = python_version.as_major_minor() else { return Vec::new() };

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
        .cartesian_product(platforms.into_iter())
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
}
