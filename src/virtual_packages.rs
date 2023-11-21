use crate::Project;
use miette::IntoDiagnostic;
use rattler_conda_types::{GenericVirtualPackage, Platform, Version};
use rattler_virtual_packages::{Archspec, LibC, Linux, Osx, VirtualPackage};
use std::collections::HashMap;

/// The default GLIBC version to use. This is used when no system requirements are specified.
pub fn default_glibc_version() -> Version {
    "2.17".parse().unwrap()
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
pub fn get_minimal_virtual_packages(platform: Platform) -> Vec<VirtualPackage> {
    // TODO: How to add a default cuda requirements
    let mut virtual_packages: Vec<VirtualPackage> = vec![];

    // Match high level platforms
    if platform.is_unix() {
        virtual_packages.push(VirtualPackage::Unix);
    }
    if platform.is_linux() {
        virtual_packages.push(VirtualPackage::Linux(Linux {
            version: "5.10".parse().unwrap(),
        }));
        virtual_packages.push(VirtualPackage::LibC(LibC {
            family: "glibc".parse().unwrap(),
            version: default_glibc_version(),
        }));
    }
    if platform.is_windows() {
        virtual_packages.push(VirtualPackage::Win);
    }

    if let Some(archspec) = Archspec::from_platform(platform) {
        virtual_packages.push(archspec.into())
    }

    // Add platform specific packages
    match platform {
        Platform::OsxArm64 => {
            virtual_packages.push(VirtualPackage::Osx(Osx {
                version: "11.0".parse().unwrap(),
            }));
        }
        Platform::Osx64 => {
            virtual_packages.push(VirtualPackage::Osx(Osx {
                version: "10.15".parse().unwrap(),
            }));
        }
        _ => {}
    }
    virtual_packages
}

/// Determines whether a virtual packages is relevant based on the platform.
pub fn non_relevant_virtual_packages_for_platform(
    requirement: &VirtualPackage,
    platform: Platform,
) -> bool {
    match platform {
        Platform::LinuxAarch64
        | Platform::Linux32
        | Platform::LinuxPpc64le
        | Platform::LinuxArmV6l
        | Platform::LinuxArmV7l
        | Platform::LinuxPpc64
        | Platform::LinuxRiscv32
        | Platform::LinuxS390X
        | Platform::LinuxRiscv64
        | Platform::Linux64 => {
            matches!(requirement, VirtualPackage::Win)
                || matches!(requirement, VirtualPackage::Osx(_))
        }
        Platform::Osx64 | Platform::OsxArm64 => {
            matches!(requirement, VirtualPackage::LibC(_))
                || matches!(requirement, VirtualPackage::Win)
                || matches!(requirement, VirtualPackage::Linux(_))
        }
        Platform::Win64 | Platform::Win32 | Platform::WinArm64 => {
            matches!(requirement, VirtualPackage::LibC(_))
                || matches!(requirement, VirtualPackage::Unix)
                || matches!(requirement, VirtualPackage::Osx(_))
                || matches!(requirement, VirtualPackage::Linux(_))
        }
        Platform::NoArch
        | Platform::Unknown
        | Platform::EmscriptenWasm32
        | Platform::WasiWasm32 => false,
    }
}

impl Project {
    /// Returns the set of virtual packages to use for the specified platform according. This method
    /// takes into account the system requirements specified in the project manifest.
    pub fn virtual_packages(
        &self,
        platform: Platform,
    ) -> miette::Result<Vec<GenericVirtualPackage>> {
        // Get the system requirements from the project manifest
        let system_requirements = self.system_requirements_for_platform(platform);

        // Combine the requirements, allowing the system requirements to overwrite the reference
        // virtual packages.
        let combined_packages = get_minimal_virtual_packages(platform)
            .into_iter()
            .chain(system_requirements)
            .map(GenericVirtualPackage::from)
            .map(|vpkg| (vpkg.name.clone(), vpkg))
            .collect::<HashMap<_, _>>();

        Ok(combined_packages.into_values().collect())
    }
}

/// Verifies if the current platform satisfies the minimal virtual package requirements.
pub fn verify_current_platform_has_required_virtual_packages(
    project: &Project,
) -> miette::Result<()> {
    let current_platform = Platform::current();

    let system_virtual_packages = VirtualPackage::current()
        .into_diagnostic()?
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();
    let required_pkgs = project.virtual_packages(current_platform)?;

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
    use crate::virtual_packages::{
        get_minimal_virtual_packages, non_relevant_virtual_packages_for_platform,
    };
    use insta::assert_debug_snapshot;
    use rattler_conda_types::Platform;
    use rattler_virtual_packages::{Archspec, LibC, Linux, Osx, VirtualPackage};

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

        for platform in platforms {
            let packages = get_minimal_virtual_packages(platform);
            let snapshot_name = format!("test_get_minimal_virtual_packages.{}", platform);
            assert_debug_snapshot!(snapshot_name, packages);
        }
    }
    #[test]
    fn test_should_retain_of_virtual_packages_on_different_os() {
        let libc = VirtualPackage::LibC(LibC {
            family: "".to_string(),
            version: "2.36".parse().unwrap(),
        });
        let win = VirtualPackage::Win;
        let osx = VirtualPackage::Osx(Osx {
            version: "11.4".parse().unwrap(),
        });
        let unix = VirtualPackage::Unix;
        let linux = VirtualPackage::Linux(Linux {
            version: "6.4.7".parse().unwrap(),
        });
        let archspec = VirtualPackage::Archspec(Archspec {
            spec: "x86_64".to_string(),
        });
        let system_requirements = vec![libc, linux, win, unix, osx, archspec];

        let linux_system_requirement: Vec<&VirtualPackage> = system_requirements
            .iter()
            .filter(|requirement| {
                !non_relevant_virtual_packages_for_platform(requirement, Platform::Linux64)
            })
            .collect();
        assert!(
            !linux_system_requirement.iter().any(|r| match r {
                VirtualPackage::Osx(_) => true,
                VirtualPackage::Win => true,
                _ => false,
            }),
            "linux has more virtual packages selected then expected: {:?}",
            linux_system_requirement
        );

        let windows_system_requirement: Vec<&VirtualPackage> = system_requirements
            .iter()
            .filter(|requirement| {
                !non_relevant_virtual_packages_for_platform(requirement, Platform::Win64)
            })
            .collect();
        assert!(!windows_system_requirement.iter().any(|r| match r {
            VirtualPackage::Osx(_) => true,
            VirtualPackage::Linux(_) => true,
            VirtualPackage::Unix => true,
            VirtualPackage::LibC(_) => true,
            _ => false,
        }));

        let osx_system_requirement: Vec<&VirtualPackage> = system_requirements
            .iter()
            .filter(|requirement| {
                !non_relevant_virtual_packages_for_platform(requirement, Platform::Osx64)
            })
            .collect();
        assert!(!osx_system_requirement.iter().any(|r| match r {
            VirtualPackage::Linux(_) => true,
            VirtualPackage::Win => true,
            VirtualPackage::LibC(_) => true,
            _ => false,
        }));
    }
}
