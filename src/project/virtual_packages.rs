use crate::project::Environment;
use pixi_default_versions::{default_glibc_version, default_linux_version, default_mac_os_version};
use pixi_manifest::{LibCSystemRequirement, SystemRequirements};
use rattler_conda_types::Platform;
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};

/// Returns a reasonable modern set of virtual packages that should be safe enough to assume.
/// At the time of writing, this is in sync with the conda-lock set of minimal virtual packages.
/// <https://github.com/conda/conda-lock/blob/3d36688278ebf4f65281de0846701d61d6017ed2/conda_lock/virtual_package.py#L175>
///
/// The method also takes into account system requirements specified in the project manifest.
pub(crate) fn get_minimal_virtual_packages(
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

    // Archspec is only based on the platform for now
    if let Some(spec) = Archspec::from_platform(platform) {
        virtual_packages.push(VirtualPackage::Archspec(spec));
    }

    virtual_packages
}

impl Environment<'_> {
    /// Returns the set of virtual packages to use for the specified platform. This method
    /// takes into account the system requirements specified in the project manifest.
    pub(crate) fn virtual_packages(&self, platform: Platform) -> Vec<VirtualPackage> {
        get_minimal_virtual_packages(platform, &self.system_requirements())
    }
}

#[cfg(test)]
mod tests {
    use crate::project::virtual_packages::get_minimal_virtual_packages;
    use insta::assert_debug_snapshot;
    use itertools::Itertools;
    use pixi_manifest::SystemRequirements;
    use rattler_conda_types::{GenericVirtualPackage, Platform};

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
            let packages = get_minimal_virtual_packages(platform, &system_requirements)
                .into_iter()
                .map(GenericVirtualPackage::from)
                .collect_vec();
            let snapshot_name = format!("test_get_minimal_virtual_packages.{}", platform);
            assert_debug_snapshot!(snapshot_name, packages);
        }
    }
}
