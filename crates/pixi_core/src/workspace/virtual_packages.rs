use crate::lock_file::virtual_packages::{
    MachineValidationError, validate_system_meets_environment_requirements,
};
use crate::workspace::{Environment, errors::UnsupportedPlatformError};
use miette::Diagnostic;
use pixi_default_versions::{
    default_glibc_version, default_linux_version, default_mac_os_version, default_windows_version,
};
use pixi_manifest::PixiPlatform;
use rattler_conda_types::{GenericVirtualPackage, Version};
use rattler_lock::LockFile;
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
use thiserror::Error;

/// Returns a reasonable modern set of virtual packages that should be safe
/// enough to assume. At the time of writing, this is in sync with the
/// conda-lock set of minimal virtual packages. <https://github.com/conda/conda-lock/blob/3d36688278ebf4f65281de0846701d61d6017ed2/conda_lock/virtual_package.py#L175>
///
/// Virtual packages declared on `platform` win; otherwise the defaults from
/// `pixi_default_versions` fill in linux/libc/win/osx. `__cuda` is only
/// included when the platform declares it.
pub(crate) fn get_minimal_virtual_packages(platform: &PixiPlatform) -> Vec<VirtualPackage> {
    let subdir = platform.subdir();
    let declared = platform.declared_virtual_packages();
    let mut virtual_packages: Vec<VirtualPackage> = vec![];

    if subdir.is_unix() {
        virtual_packages.push(VirtualPackage::Unix);
    }
    if subdir.is_linux() {
        let version = declared_version(declared, "__linux").unwrap_or_else(default_linux_version);
        virtual_packages.push(VirtualPackage::Linux(Linux { version }));

        let (family, version) = declared_libc(declared)
            .unwrap_or_else(|| ("glibc".to_string(), default_glibc_version()));
        virtual_packages.push(VirtualPackage::LibC(LibC { family, version }));
    }

    if subdir.is_windows() {
        let version =
            declared_version(declared, "__win").or_else(|| Some(default_windows_version()));
        virtual_packages.push(VirtualPackage::Win(rattler_virtual_packages::Windows {
            version,
        }));
    }

    if subdir.is_osx() {
        let version =
            declared_version(declared, "__osx").unwrap_or_else(|| default_mac_os_version(subdir));
        virtual_packages.push(VirtualPackage::Osx(Osx { version }));
    }

    if let Some(version) = declared_version(declared, "__cuda") {
        virtual_packages.push(VirtualPackage::Cuda(Cuda { version }));
    }

    // Archspec is still subdir-derived: rattler's Archspec needs a microarch
    // database lookup, not just a string from the manifest.
    if let Some(spec) = Archspec::from_platform(subdir) {
        virtual_packages.push(VirtualPackage::Archspec(spec));
    }

    virtual_packages
}

fn declared_version(declared: &[GenericVirtualPackage], name: &str) -> Option<Version> {
    declared
        .iter()
        .find(|gvp| gvp.name.as_normalized() == name)
        .map(|gvp| gvp.version.clone())
}

fn declared_libc(declared: &[GenericVirtualPackage]) -> Option<(String, Version)> {
    declared.iter().find_map(|gvp| {
        let family = match gvp.name.as_normalized() {
            "__glibc" => "glibc",
            "__musl" => "musl",
            "__eglibc" => "eglibc",
            _ => return None,
        };
        Some((family.to_string(), gvp.version.clone()))
    })
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
    lock_file: Option<&LockFile>,
) -> Result<(), VerifyCurrentPlatformError> {
    // When overriding platform skip validation entirely.
    // The host platform wouldn't satisfy the requirements
    if std::env::var(pixi_consts::consts::PIXI_OVERRIDE_PLATFORM).is_ok() {
        return Ok(());
    }

    let Some(current_platform) = environment.best_platform() else {
        return Err(VerifyCurrentPlatformError::from(Box::new(
            environment.unsupported_platform_error(),
        )));
    };

    // If this function is given a lock file we can also compute the ability to run in this environment on the current machine.
    if let Some(lock_file) = lock_file {
        validate_system_meets_environment_requirements(
            lock_file,
            current_platform,
            environment.name(),
            None,
        )?;
    }

    Ok(())
}
impl Environment<'_> {
    /// Returns the set of virtual packages to use for the specified platform.
    /// Virtual-package versions are taken from `platform`'s declarations,
    /// with `pixi_default_versions` filling in linux/libc/win/osx defaults.
    pub fn virtual_packages(&self, platform: &PixiPlatform) -> Vec<VirtualPackage> {
        get_minimal_virtual_packages(platform)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use insta::assert_debug_snapshot;
    use itertools::Itertools;
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

        for platform in platforms {
            let pp = pixi_manifest::PixiPlatform::from_subdir(platform);
            let packages = get_minimal_virtual_packages(&pp)
                .into_iter()
                .map(GenericVirtualPackage::from)
                .collect_vec();
            insta::with_settings!({snapshot_suffix => platform.as_str()}, {
                assert_debug_snapshot!(packages);
            });
        }
    }

    #[test]
    fn declared_cuda_overrides_default() {
        let mut pp = pixi_manifest::PixiPlatform::new(
            pixi_manifest::PixiPlatformName::try_from("gpu").unwrap(),
            Platform::Linux64,
            vec![GenericVirtualPackage {
                name: rattler_conda_types::PackageName::try_from("__cuda").unwrap(),
                version: rattler_conda_types::Version::from_str("12.0").unwrap(),
                build_string: String::new(),
            }],
        )
        .unwrap();
        let packages = get_minimal_virtual_packages(&pp);
        let cuda = packages
            .iter()
            .find_map(|vp| match vp {
                VirtualPackage::Cuda(c) => Some(c.version.clone()),
                _ => None,
            })
            .expect("__cuda should be present");
        assert_eq!(cuda.to_string(), "12.0");

        // Without declaration, cuda is absent.
        pp.set_declared_virtual_packages(Vec::new()).ok();
        // Note: set_declared_virtual_packages errors on subdir-platforms; in
        // this case the name "gpu" != subdir "linux-64" so it succeeds. A
        // platform with no declared cuda should not emit a __cuda VP.
        let bare = pixi_manifest::PixiPlatform::from_subdir(Platform::Linux64);
        assert!(
            !get_minimal_virtual_packages(&bare)
                .iter()
                .any(|vp| matches!(vp, VirtualPackage::Cuda(_))),
            "bare subdir platform should not declare __cuda"
        );
    }

    #[test]
    fn declared_libc_picks_family_and_version() {
        let pp = pixi_manifest::PixiPlatform::new(
            pixi_manifest::PixiPlatformName::try_from("musl-host").unwrap(),
            Platform::LinuxAarch64,
            vec![GenericVirtualPackage {
                name: rattler_conda_types::PackageName::try_from("__musl").unwrap(),
                version: rattler_conda_types::Version::from_str("1.2.4").unwrap(),
                build_string: String::new(),
            }],
        )
        .unwrap();
        let libc = get_minimal_virtual_packages(&pp)
            .into_iter()
            .find_map(|vp| match vp {
                VirtualPackage::LibC(l) => Some(l),
                _ => None,
            })
            .expect("LibC VP should be present");
        assert_eq!(libc.family, "musl");
        assert_eq!(libc.version.to_string(), "1.2.4");
    }
}
