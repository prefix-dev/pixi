use crate::lock_file::virtual_packages::{
    MachineValidationError, compute_minimal_required_platforms,
    validate_system_meets_environment_requirements,
};
use crate::workspace::environment::{
    current_platform_with_override, detect_system_virtual_packages,
};
use crate::workspace::{Environment, errors::UnsupportedPlatformError};
use fancy_display::FancyDisplay;
use miette::Diagnostic;
use pixi_manifest::{FeaturesExt, HasWorkspaceManifest, PixiPlatform};
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

/// Verifies that the current machine can run `environment`.
///
/// Two checks, in order:
///
/// 1. *Declared compatibility* -- does one of the environment's declared
///    platforms match this machine (subdir + declared virtual packages)? This
///    is [`Environment::best_platform`].
/// 2. *Resolution compatibility* -- if (1) fails and a resolution is available,
///    fall back to the minimal-required platform derived from the resolved
///    dependencies (a declared platform may promise virtual packages the
///    resolved packages don't actually need). If the machine satisfies that
///    minimal set, the environment can run.
///
/// Outcomes: (1) holds -> ok; (1) fails but (2) holds -> ok with a warning;
/// both fail -> error listing the unmet minimal requirements.
pub fn verify_current_platform_can_run_environment(
    environment: &Environment<'_>,
    lock_file: Option<&LockFile>,
) -> Result<(), VerifyCurrentPlatformError> {
    // When overriding platform skip validation entirely.
    // The host platform wouldn't satisfy the requirements
    if std::env::var(pixi_consts::consts::PIXI_OVERRIDE_PLATFORM).is_ok() {
        return Ok(());
    }

    // Check 1: a declared platform matches this machine.
    if let Some(current_platform) = environment.best_platform() {
        // Declared-compatible. Keep validating the resolved requirements
        // (conda virtual packages + pypi wheel tags) against the lock file.
        if let Some(lock_file) = lock_file {
            validate_system_meets_environment_requirements(
                lock_file,
                current_platform,
                environment.name(),
                None,
            )?;
        }
        return Ok(());
    }

    // Check 1 failed. Without a resolution there is nothing to fall back on, so
    // keep the original "platform not supported" error.
    let Some(lock_file) = lock_file else {
        return Err(VerifyCurrentPlatformError::from(Box::new(
            environment.unsupported_platform_error(),
        )));
    };

    // Check 2: does the machine satisfy the minimal-required platform for a
    // subdir it can run (the current subdir or an architecture fallback)?
    let current = current_platform_with_override();
    let system_virtual_packages = detect_system_virtual_packages();
    let candidate_subdirs = environment
        .workspace_manifest()
        .workspace
        .candidate_subdirs(current);

    let manifest = environment.workspace_manifest();
    let declared_platforms: Vec<&PixiPlatform> = environment
        .platforms()
        .iter()
        .filter_map(|name| manifest.workspace.platform_by_name(name))
        .collect();
    let minimal =
        compute_minimal_required_platforms(lock_file, environment.name(), &declared_platforms);

    let mut unmet: Option<Vec<GenericVirtualPackage>> = None;
    for subdir in &candidate_subdirs {
        let Some(platform) = minimal.get(subdir) else {
            continue;
        };
        let unsatisfied = unsatisfied_virtual_packages(platform, &system_virtual_packages);
        if unsatisfied.is_empty() {
            // Check 1 failed but the resolution is compatible -- continue.
            tracing::warn!(
                "The current machine is not one of the platforms declared for environment '{}', but the resolved dependencies are compatible with it -- continuing.",
                environment.name().fancy_display(),
            );
            return Ok(());
        }
        unmet.get_or_insert(unsatisfied);
    }

    // Both checks failed: report the unmet minimal requirements.
    Err(VerifyCurrentPlatformError::from(Box::new(
        UnsupportedPlatformError {
            environments_platforms: environment.platforms().into_iter().collect(),
            environment: environment.name().clone(),
            platform: current,
            unsatisfied_requirements: unmet.unwrap_or_default(),
        },
    )))
}

/// The declared virtual packages of `platform` that the machine does not
/// provide (missing entirely, or present at a lower version).
fn unsatisfied_virtual_packages(
    platform: &PixiPlatform,
    system: &[GenericVirtualPackage],
) -> Vec<GenericVirtualPackage> {
    platform
        .declared_virtual_packages()
        .iter()
        .filter(|required| {
            !system
                .iter()
                .any(|sys| sys.name == required.name && sys.version >= required.version)
        })
        .cloned()
        .collect()
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
    fn unsatisfied_virtual_packages_reports_missing_and_lower() {
        use rattler_conda_types::{PackageName, Version};

        let platform = pixi_manifest::PixiPlatform::from_required_virtual_packages(
            Platform::Linux64,
            vec![GenericVirtualPackage {
                name: PackageName::try_from("__cuda").unwrap(),
                version: Version::from_str("12").unwrap(),
                build_string: String::new(),
            }],
        );
        let cuda = |v: &str| {
            vec![GenericVirtualPackage {
                name: PackageName::try_from("__cuda").unwrap(),
                version: Version::from_str(v).unwrap(),
                build_string: String::new(),
            }]
        };

        // Machine provides cuda 12 -> the requirement is met.
        assert!(unsatisfied_virtual_packages(&platform, &cuda("12")).is_empty());
        // A higher machine version still satisfies the minimum.
        assert!(unsatisfied_virtual_packages(&platform, &cuda("12.4")).is_empty());
        // A lower machine version leaves the requirement unmet.
        let unmet = unsatisfied_virtual_packages(&platform, &cuda("11"));
        assert_eq!(unmet.len(), 1);
        assert_eq!(unmet[0].name.as_normalized(), "__cuda");
        // No cuda at all -> unmet.
        assert_eq!(unsatisfied_virtual_packages(&platform, &[]).len(), 1);
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
