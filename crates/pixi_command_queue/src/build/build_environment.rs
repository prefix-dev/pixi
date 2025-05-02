use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_virtual_packages::{
    DetectVirtualPackageError, VirtualPackageOverrides, VirtualPackages,
};

/// Contains information about the build and host environments.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct BuildEnvironment {
    pub host_platform: Platform,
    pub host_virtual_packages: Vec<GenericVirtualPackage>,
    pub build_platform: Platform,
    pub build_virtual_packages: Vec<GenericVirtualPackage>,
}

impl Default for BuildEnvironment {
    fn default() -> Self {
        let virtual_packages = VirtualPackages::default()
            .into_generic_virtual_packages()
            .collect::<Vec<_>>();

        Self {
            host_platform: Platform::current(),
            host_virtual_packages: virtual_packages.clone(),
            build_platform: Platform::current(),
            build_virtual_packages: virtual_packages,
        }
    }
}

impl BuildEnvironment {
    /// Constructs a build environment that targets a specific `target_platform` from the current platform.
    pub fn simple_cross(target_platform: Platform) -> Result<Self, DetectVirtualPackageError> {
        Ok(Self {
            host_platform: target_platform,
            host_virtual_packages: vec![],
            build_platform: Platform::current(),
            build_virtual_packages: VirtualPackages::detect(&VirtualPackageOverrides::default())?
                .into_generic_virtual_packages()
                .collect(),
        })
    }
}
