use crate::lock_file::virtual_packages::{
    MachineValidationError, validate_system_meets_environment_requirements,
};
use crate::workspace::{Environment, errors::UnsupportedPlatformError};
use miette::Diagnostic;
use pixi_manifest::PixiPlatform;
use rattler_conda_types::GenericVirtualPackage;
use rattler_lock::LockFile;
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
use thiserror::Error;

/// Convert a [`PixiPlatform`]'s declared virtual packages into the typed
/// [`VirtualPackage`] form rattler's solver wants.
///
/// The subdir baseline is no longer recomputed here: every real subdir or
/// rich platform already carries the materialised defaults via
/// [`PixiPlatform::from_subdir`] / [`PixiPlatform::new_with_defaults`], and
/// the only platform that intentionally has an empty declared list is the
/// `auto_detected` host-display placeholder, which never reaches this path.
/// The result mirrors what the conda-lock minimal-virtual-package set used
/// to spell out by hand. <https://github.com/conda/conda-lock/blob/3d36688278ebf4f65281de0846701d61d6017ed2/conda_lock/virtual_package.py#L175>
///
/// Unknown conda virtual-package names (those rattler has no typed slot for)
/// are dropped -- they round-trip through the manifest but never influence
/// solving directly, the same behavior the previous implementation had.
pub(crate) fn get_minimal_virtual_packages(platform: &PixiPlatform) -> Vec<VirtualPackage> {
    platform
        .declared_virtual_packages()
        .iter()
        .filter_map(generic_to_virtual_package)
        .collect()
}

/// Translate a single [`GenericVirtualPackage`] into the typed
/// [`VirtualPackage`] variant rattler expects. Returns `None` for entries
/// that don't have a typed counterpart (rattler-unknown `__*` names).
fn generic_to_virtual_package(gvp: &GenericVirtualPackage) -> Option<VirtualPackage> {
    match gvp.name.as_normalized() {
        "__unix" => Some(VirtualPackage::Unix),
        "__linux" => Some(VirtualPackage::Linux(Linux {
            version: gvp.version.clone(),
        })),
        family @ ("__glibc" | "__musl" | "__eglibc") => Some(VirtualPackage::LibC(LibC {
            family: family.trim_start_matches('_').to_string(),
            version: gvp.version.clone(),
        })),
        "__win" => Some(VirtualPackage::Win(rattler_virtual_packages::Windows {
            version: Some(gvp.version.clone()),
        })),
        "__osx" => Some(VirtualPackage::Osx(Osx {
            version: gvp.version.clone(),
        })),
        "__cuda" => Some(VirtualPackage::Cuda(Cuda {
            version: gvp.version.clone(),
        })),
        "__archspec" => {
            // Rattler maps an archspec string through a microarch database
            // lookup; an empty/"0" build-string means "unknown microarch"
            // and `from_name` returns the generic catch-all in that case.
            let name = if gvp.build_string.is_empty() || gvp.build_string == "0" {
                return Some(VirtualPackage::Archspec(Archspec::Unknown));
            } else {
                gvp.build_string.as_str()
            };
            Some(VirtualPackage::Archspec(Archspec::from_name(name)))
        }
        _ => None,
    }
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
    /// Reads them straight off `platform.declared_virtual_packages()`: the
    /// subdir baseline is materialised by [`PixiPlatform::from_subdir`], so
    /// there is no separate "compute defaults" step.
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
