mod work_dir_key;

use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_virtual_packages::VirtualPackages;
pub(crate) use work_dir_key::WorkDirKey;

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
