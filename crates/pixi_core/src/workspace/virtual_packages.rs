use crate::lock_file::virtual_packages::{
    MachineValidationError, validate_system_meets_environment_requirements,
};
use crate::workspace::{Environment, errors::UnsupportedPlatformError};
use itertools::Itertools;
use miette::Diagnostic;
use pixi_default_versions::{
    default_glibc_version, default_linux_version, default_mac_os_version, default_windows_version,
};
use pixi_manifest::{FeaturesExt, LibCSystemRequirement, SystemRequirements};
use rattler_conda_types::Platform;
use rattler_lock::LockFile;
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
use thiserror::Error;

/// Returns a reasonable modern set of virtual packages that should be safe
/// enough to assume. At the time of writing, this is in sync with the
/// conda-lock set of minimal virtual packages. <https://github.com/conda/conda-lock/blob/3d36688278ebf4f65281de0846701d61d6017ed2/conda_lock/virtual_package.py#L175>
///
/// The method also takes into account system requirements specified in the
/// project manifest.
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
        // todo: add windows to system requirements
        let version = Some(default_windows_version());
        virtual_packages.push(VirtualPackage::Win(rattler_virtual_packages::Windows {
            version,
        }));
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

/// An error that occurs when the current platform does not satisfy the minimal virtual package
/// requirements.
#[derive(Debug, Error, Diagnostic)]
pub enum VerifyCurrentPlatformError {
    #[error("The current platform does not satisfy the minimal virtual package requirements")]
    UnsupportedPlatform(#[from] Box<UnsupportedPlatformError>),

    #[error(transparent)]
    MachineValidationError(#[from] MachineValidationError),
}

/// Verifies if the current platform satisfies the minimal virtual package
/// requirements.
pub fn verify_current_platform_can_run_environment(
    environment: &Environment<'_>,
    lockfile: Option<&LockFile>,
) -> Result<(), VerifyCurrentPlatformError> {
    let current_platform = environment.best_platform();

    // Are there dependencies and is the current platform in the list of supported platforms?
    if !environment.platforms().contains(&current_platform) {
        return Err(VerifyCurrentPlatformError::from(Box::new(
            UnsupportedPlatformError {
                environments_platforms: environment.platforms().into_iter().collect_vec(),
                platform: current_platform,
                environment: environment.name().clone(),
            },
        )));
    }

    // If this function is given a lockfile we can also compute the ability to run in this environment on the current machine.
    if let Some(lockfile) = lockfile {
        validate_system_meets_environment_requirements(
            lockfile,
            current_platform,
            environment.name(),
            None,
        )?;
    }

    Ok(())
}
impl Environment<'_> {
    /// Returns the set of virtual packages to use for the specified platform. This method
    /// takes into account the system requirements specified in the project manifest.
    pub fn virtual_packages(&self, platform: Platform) -> Vec<VirtualPackage> {
        get_minimal_virtual_packages(platform, &self.system_requirements())
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_debug_snapshot;
    use itertools::Itertools;
    use pixi_manifest::SystemRequirements;
    use rattler_conda_types::{GenericVirtualPackage, Platform};

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

        let system_requirements = SystemRequirements::default();

        for platform in platforms {
            let packages = get_minimal_virtual_packages(platform, &system_requirements)
                .into_iter()
                .map(GenericVirtualPackage::from)
                .collect_vec();
            insta::with_settings!({snapshot_suffix => platform.as_str()}, {
                assert_debug_snapshot!(packages);
            });
        }
    }
}
