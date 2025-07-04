use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_virtual_packages::{
    DetectVirtualPackageError, VirtualPackageOverrides, VirtualPackages,
};
use serde::Serialize;

/// Contains information about the build and host environments.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize)]
pub struct BuildEnvironment {
    pub host_platform: Platform,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub host_virtual_packages: Vec<GenericVirtualPackage>,
    pub build_platform: Platform,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub build_virtual_packages: Vec<GenericVirtualPackage>,
}

impl BuildEnvironment {
    /// Constructs a new build environment where the host environment is the same as the build environment.
    pub fn to_build_from_build(&self) -> Self {
        Self {
            host_platform: self.build_platform,
            host_virtual_packages: self.build_virtual_packages.clone(),
            build_platform: self.build_platform,
            build_virtual_packages: self.build_virtual_packages.clone(),
        }
    }
}

impl Default for BuildEnvironment {
    fn default() -> Self {
        let virtual_packages: Vec<_> = VirtualPackages::detect(&VirtualPackageOverrides::default())
            .unwrap_or_default()
            .into_generic_virtual_packages()
            .collect();

        Self {
            host_platform: Platform::current(),
            host_virtual_packages: virtual_packages.clone(),
            build_platform: Platform::current(),
            build_virtual_packages: virtual_packages,
        }
    }
}

impl BuildEnvironment {
    /// Constructs a build environment that targets a specific `target_platform`
    /// from the current platform.
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

    /// Constructs a build environment that targets a specific `target_platform`
    pub fn simple(platform: Platform, virtual_packages: Vec<GenericVirtualPackage>) -> Self {
        Self {
            host_platform: platform,
            host_virtual_packages: virtual_packages.clone(),
            build_platform: platform,
            build_virtual_packages: virtual_packages,
        }
    }
}
