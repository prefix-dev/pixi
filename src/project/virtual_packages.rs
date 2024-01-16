use super::manifest::{LibCSystemRequirement, SystemRequirements};
use crate::project::Environment;
use miette::IntoDiagnostic;
use rattler_conda_types::{GenericVirtualPackage, Platform, Version};
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
use std::collections::HashMap;

/// The default GLIBC version to use. This is used when no system requirements are specified.
pub fn default_glibc_version() -> Version {
    "2.17".parse().unwrap()
}

/// The default linux version to use. This is used when no system requirements are specified.
pub fn default_linux_version() -> Version {
    "5.10".parse().unwrap()
}

/// Returns the default Mac OS version for the specified platform. The platform must refer to a
/// MacOS platform.
pub fn default_mac_os_version(platform: Platform) -> Version {
    match platform {
        Platform::OsxArm64 => "11.0".parse().unwrap(),
        Platform::Osx64 => "10.15".parse().unwrap(),
        _ => panic!(
            "default_mac_os_version() called with non-osx platform: {}",
            platform
        ),
    }
}

/// Returns a reasonable modern set of virtual packages that should be safe enough to assume.
/// At the time of writing, this is in sync with the conda-lock set of minimal virtual packages.
/// <https://github.com/conda/conda-lock/blob/3d36688278ebf4f65281de0846701d61d6017ed2/conda_lock/virtual_package.py#L175>
///
/// The method also takes into account system requirements specified in the project manifest.
pub fn get_minimal_virtual_packages(
    platform: Platform,
    system_requirements: &SystemRequirements,
) -> Vec<VirtualPackage> {
    // TODO: How to add a default cuda requirements
    let mut virtual_packages: Vec<VirtualPackage> = vec![];

    // Match high level platforms
    if platform.is_unix() {
        virtual_packages.push(VirtualPackage::Unix);
    }
    if platform.is_linux() {
        let version = system_requirements
            .linux
            .clone()
            .unwrap_or(default_linux_version());
        virtual_packages.push(VirtualPackage::Linux(Linux { version }));

        let (family, version) = system_requirements
            .libc
            .as_ref()
            .map(LibCSystemRequirement::family_and_version)
            .map(|(family, version)| (family.to_string(), version.clone()))
            .unwrap_or(("glibc".parse().unwrap(), default_glibc_version()));
        virtual_packages.push(VirtualPackage::LibC(LibC { family, version }));
    }

    if platform.is_windows() {
        virtual_packages.push(VirtualPackage::Win);
    }

    if let Some(archspec) = system_requirements.archspec.clone() {
        virtual_packages.push(VirtualPackage::Archspec(Archspec { spec: archspec }))
    } else if let Some(archspec) = Archspec::from_platform(platform) {
        virtual_packages.push(archspec.into())
    }

    // Add platform specific packages
    if platform.is_osx() {
        let version = system_requirements
            .macos
            .clone()
            .unwrap_or_else(|| default_mac_os_version(platform));
        virtual_packages.push(VirtualPackage::Osx(Osx { version }));
    }

    // Cuda
    if let Some(version) = system_requirements.cuda.clone() {
        virtual_packages.push(VirtualPackage::Cuda(Cuda { version }));
    }

    virtual_packages
}

impl Environment<'_> {
    /// Returns the set of virtual packages to use for the specified platform. This method
    /// takes into account the system requirements specified in the project manifest.
    pub fn virtual_packages(&self, platform: Platform) -> Vec<GenericVirtualPackage> {
        get_minimal_virtual_packages(platform, &self.system_requirements())
            .into_iter()
            .map(GenericVirtualPackage::from)
            .collect()
    }
}

/// Verifies if the current platform satisfies the minimal virtual package requirements.
pub fn verify_current_platform_has_required_virtual_packages(
    environment: &Environment<'_>,
) -> miette::Result<()> {
    let current_platform = Platform::current();

    let system_virtual_packages = VirtualPackage::current()
        .into_diagnostic()?
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();
    let required_pkgs = environment.virtual_packages(current_platform);

    // Check for every local minimum package if it is available and on the correct version.
    for req_pkg in required_pkgs {
        if let Some(local_vpkg) = system_virtual_packages.get(&req_pkg.name) {
            if req_pkg.build_string != local_vpkg.build_string {
                miette::bail!("The current system has a mismatching virtual package. The project requires '{}' to be on build '{}' but the system has build '{}'", req_pkg.name.as_source(), req_pkg.build_string, local_vpkg.build_string);
            }

            if req_pkg.version > local_vpkg.version {
                // This case can simply happen because the default system requirements in get_minimal_virtual_packages() is higher than required.
                miette::bail!("The current system has a mismatching virtual package. The project requires '{}' to be at least version '{}' but the system has version '{}'\n\n\
                Try setting the following in your pixi.toml:\n\
                [system-requirements]\n\
                {} = \"{}\"", req_pkg.name.as_source(), req_pkg.version, local_vpkg.version, req_pkg.name.as_normalized().strip_prefix("__").unwrap_or(local_vpkg.name.as_normalized()), local_vpkg.version);
            }
        } else {
            miette::bail!("The platform you are running on should at least have the virtual package {} on version {}, build_string: {}", req_pkg.name.as_source(), req_pkg.version, req_pkg.build_string)
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::manifest::SystemRequirements;
    use insta::assert_debug_snapshot;
    use rattler_conda_types::Platform;

    // Regression test on the virtual packages so there is not accidental changes
    #[test]
    fn test_get_minimal_virtual_packages() {
        let platforms = vec![
            Platform::NoArch,
            Platform::Linux64,
            Platform::LinuxAarch64,
            Platform::LinuxPpc64le,
            Platform::Osx64,
            Platform::OsxArm64,
            Platform::Win64,
        ];

        let system_requirements = SystemRequirements::default();

        for platform in platforms {
            let packages = get_minimal_virtual_packages(platform, &system_requirements);
            let snapshot_name = format!("test_get_minimal_virtual_packages.{}", platform);
            assert_debug_snapshot!(snapshot_name, packages);
        }
    }
}
