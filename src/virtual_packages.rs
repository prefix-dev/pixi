use anyhow::bail;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_virtual_packages::{Archspec, LibC, Linux, Osx, VirtualPackage};

/// Define a reasonable modern set of virtual packages that should be safe enough to assume.
/// On design this is in sync with the conda-lock set of default packages.
/// https://github.com/conda/conda-lock/blob/3d36688278ebf4f65281de0846701d61d6017ed2/conda_lock/virtual_package.py#L175
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
            family: "glib".parse().unwrap(),
            version: "2.17".parse().unwrap(),
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

pub fn verify_current_platform_has_minimal_virtual_package_requirements(
    local_platform: Platform,
) -> Result<(), anyhow::Error> {
    let local_vpkgs = VirtualPackage::current().map(|vpkgs| {
        vpkgs
            .iter()
            .map(|vpkg| GenericVirtualPackage::from(vpkg.clone()))
            .collect::<Vec<_>>()
    })?;

    let local_minimal_vpkgs: Vec<GenericVirtualPackage> =
        get_minimal_virtual_packages(local_platform)
            .iter()
            .map(|vpkg| GenericVirtualPackage::from(vpkg.clone()))
            .collect();

    // Check for every local minimum package if it is available and on the correct version.
    for local_min in local_minimal_vpkgs {
        if let Some(local_vpkg) = local_vpkgs
            .iter()
            .find(|&pkg| pkg.name == local_min.name && pkg.build_string == local_min.build_string)
        {
            if local_min.version > local_vpkg.version {
                bail!("The platform you are running on does not contain the minimal version ({}) of the virtual package {}, overwrite it or use newer system for this package.", local_min.version, local_min.name)
            }
        } else {
            bail!("The platform you are running on should at least have the virtual package: {} on version: {} and build_string: {}", local_min.name, local_min.version, local_min.build_string)
        }
    }

    Ok(())
}

mod tests {
    use insta::assert_debug_snapshot;
    use super::*;

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
            assert_debug_snapshot!(packages);
        }
    }
}
