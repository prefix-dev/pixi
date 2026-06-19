use crate::environment::PlatformData;
use crate::lock_file::virtual_packages::{
    MachineValidationError, compute_minimal_required_platforms,
    validate_system_meets_environment_requirements,
};
use crate::workspace::{
    Environment, HasWorkspaceRef, PlatformOverrides, PlatformSource,
    errors::UnsupportedPlatformError,
};
use fancy_display::FancyDisplay;
use miette::Diagnostic;
use pixi_manifest::{
    EnvironmentName, FeaturesExt, HasWorkspaceManifest, PixiPlatform, PixiPlatformName,
};
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_lock::LockFile;
use rattler_virtual_packages::{Archspec, Cuda, CudaArch, LibC, Linux, Osx, VirtualPackage};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
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
        "__cuda_arch" => Some(VirtualPackage::CudaArch(CudaArch {
            version: gvp.version.clone(),
        })),
        "__archspec" => {
            // Rattler maps an archspec string through a microarch database
            // lookup; an empty/"0" build-string means "unknown microarch"
            // and `from_name` returns the generic catch-all in that case.
            if gvp.build_string.is_empty() || gvp.build_string == "0" {
                return Some(VirtualPackage::Archspec(Archspec::Unknown));
            }
            Some(VirtualPackage::Archspec(Archspec::from_name(
                gvp.build_string.as_str(),
            )))
        }
        _ => None,
    }
}

/// An error that occurs when the current platform does not satisfy the minimal virtual package
/// requirements.
#[derive(Debug, Error, Diagnostic)]
pub enum VerifyCurrentPlatformError {
    #[error(transparent)]
    #[diagnostic(transparent)]
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
///    is [`Environment::best_declared_platform`].
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
    if let Some(current_platform) = environment.best_declared_platform() {
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
    match minimum_compatible_declared_platform(environment, lock_file) {
        Ok(_) => {
            // Check 1 failed but the resolution is compatible -- continue.
            tracing::warn!(
                "The current machine is not one of the platforms declared for environment '{}', but the resolved dependencies are compatible with it -- continuing.",
                environment.name().fancy_display(),
            );
            Ok(())
        }
        // Both checks failed: report the unmet minimal requirements.
        Err(unmet) => Err(VerifyCurrentPlatformError::from(Box::new(
            UnsupportedPlatformError {
                environments_platforms: environment.platforms().into_iter().collect(),
                environment: environment.name().clone(),
                platform: environment
                    .workspace()
                    .host_platform(
                        PlatformSource::Defaults,
                        PlatformOverrides::EnvironmentVariableOverrides,
                    )
                    .subdir(),
                unsatisfied_requirements: unmet,
            },
        ))),
    }
}

/// The declared platform an environment can run on "by accident": none of
/// the declared platforms' virtual packages are satisfied by this machine,
/// but the lock-resolved minimum requirements for a subdir the machine can
/// run are. Returns the declared platform install should target, or the
/// unmet minimal requirements when the machine falls below the minimum too.
pub fn minimum_compatible_declared_platform<'p>(
    environment: &Environment<'p>,
    lock_file: &LockFile,
) -> Result<&'p PixiPlatform, Vec<GenericVirtualPackage>> {
    let current = environment
        .workspace()
        .host_platform(
            PlatformSource::Defaults,
            PlatformOverrides::EnvironmentVariableOverrides,
        )
        .subdir();
    let system_virtual_packages = environment
        .workspace()
        .host_platform(
            PlatformSource::AutoDetected,
            PlatformOverrides::EnvironmentVariableOverrides,
        )
        .declared_virtual_packages()
        .to_vec();
    let candidate_subdirs = environment
        .workspace_manifest()
        .workspace
        .candidate_subdirs(current);

    let manifest = environment.workspace_manifest();
    let env_platform_names = environment.platforms();
    // Workspace declaration order, so ties between declared platforms that
    // share a subdir resolve deterministically.
    let declared_platforms: Vec<&PixiPlatform> = manifest
        .workspace
        .platforms
        .iter()
        .filter(|platform| env_platform_names.contains(platform.name()))
        .collect();
    let minimal =
        compute_minimal_required_platforms(lock_file, environment.name(), &declared_platforms);

    let mut unmet: Option<Vec<GenericVirtualPackage>> = None;
    for subdir in &candidate_subdirs {
        // A subdir with no resolved packages requires no virtual packages, so
        // the machine trivially satisfies it -- e.g. an empty environment whose
        // only content is tasks still runs under an unsatisfiable requirement.
        let unsatisfied = minimal
            .get(subdir)
            .map(|platform| unsatisfied_virtual_packages(platform, &system_virtual_packages))
            .unwrap_or_default();
        if unsatisfied.is_empty() {
            if let Some(declared) = declared_platforms
                .iter()
                .find(|declared| declared.subdir() == *subdir)
            {
                return Ok(declared);
            }
            continue;
        }
        unmet.get_or_insert(unsatisfied);
    }

    Err(unmet.unwrap_or_default())
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

/// The platform the environment was installed for cannot run the installed
/// packages: they require virtual packages this platform does not provide.
#[derive(Debug, Error, Diagnostic)]
#[error("the installed environment '{environment}' cannot run on platform '{platform}'")]
#[diagnostic(help(
    "The installed packages require virtual packages this platform does not provide: [{}]. Reinstall for this machine with 'pixi install', or select a compatible platform with '--platform'.",
    .unmet.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
))]
pub struct RunPlatformUnsupportedError {
    environment: EnvironmentName,
    platform: PixiPlatformName,
    unmet: Vec<GenericVirtualPackage>,
}

/// How a base platform compares to the resolved/minimum platforms an
/// environment was installed for.
#[derive(Debug, PartialEq, Eq)]
enum RunPlatformVerdict {
    /// The base meets the resolution platform: it runs as intended.
    Compatible,
    /// The base meets only the minimum requirements, not the full resolution
    /// platform: it runs, but the environment was resolved for more.
    OnlyMinimum,
    /// The base is below the minimum: the installed packages cannot run, with
    /// the virtual packages it fails to provide.
    BelowMinimum(Vec<GenericVirtualPackage>),
}

/// The virtual packages `required` needs that `base_capabilities` does not
/// provide (missing, or present at a lower version). Subdir-agnostic.
fn unmet_virtual_packages(
    required: &PlatformData,
    base_capabilities: &[GenericVirtualPackage],
) -> Vec<GenericVirtualPackage> {
    let required_platform = PixiPlatform::from_required_virtual_packages(
        required.subdir(),
        required.virtual_packages().to_vec(),
    );
    unsatisfied_virtual_packages(&required_platform, base_capabilities)
}

/// Classify a base platform against the resolution and minimum platforms an
/// environment was installed for (read from `conda-meta/pixi`). `base_subdirs`
/// are the subdirs the base can run (a single subdir for an explicit
/// `--platform`, or the host's candidate subdirs incl. architecture
/// fallbacks); a required subdir outside that set never satisfies.
fn classify_run_platform(
    base_subdirs: &[Platform],
    base_capabilities: &[GenericVirtualPackage],
    resolved: &PlatformData,
    minimum: &PlatformData,
) -> RunPlatformVerdict {
    let satisfies = |required: &PlatformData| {
        base_subdirs.contains(&required.subdir())
            && unmet_virtual_packages(required, base_capabilities).is_empty()
    };

    if satisfies(resolved) {
        RunPlatformVerdict::Compatible
    } else if satisfies(minimum) {
        RunPlatformVerdict::OnlyMinimum
    } else if base_subdirs.contains(&minimum.subdir()) {
        RunPlatformVerdict::BelowMinimum(unmet_virtual_packages(minimum, base_capabilities))
    } else {
        RunPlatformVerdict::BelowMinimum(minimum.virtual_packages().to_vec())
    }
}

/// Marker-file paths we've already emitted the "runs by accident" warning for
/// in this process, so a multi-task run warns at most once per environment.
static BY_ACCIDENT_WARNED: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Warn -- at most once per process and once ever per workspace -- that
/// `environment` runs on `base` only because its minimum requirements happen to
/// be met, not the platform it was resolved for. Mirrors the persisted
/// one-time-message scheme used by [`Environment::emit_emulation_warning`].
fn warn_runs_by_accident(environment: &Environment<'_>, base: &PixiPlatformName) {
    let marker = environment
        .workspace()
        .pixi_dir()
        .join(pixi_consts::consts::ONE_TIME_MESSAGES_DIR)
        .join(format!("runs-by-accident-{}", environment.name()));

    // Dedup within this process (and short-circuit the on-disk check below).
    let Ok(mut warned) = BY_ACCIDENT_WARNED.lock() else {
        return;
    };
    if !warned.insert(marker.clone()) {
        return;
    }
    drop(warned);

    // A previous run already warned for this workspace + environment.
    if marker.exists() {
        return;
    }

    tracing::warn!(
        "Environment '{}' was resolved for a richer platform than '{}' provides; this machine only meets the installed packages' minimum requirements, so it runs here by accident.",
        environment.name().fancy_display(),
        base,
    );

    // Persist the marker so future runs stay quiet. Best-effort.
    if let Some(parent) = marker.parent() {
        let _ = fs_err::create_dir_all(parent).and_then(|()| fs_err::File::create(&marker));
    }
}

/// Verify that the platform we are about to run tasks on can actually run the
/// installed environment, using the resolved and minimum platforms recorded in
/// the environment's `conda-meta/pixi` marker.
///
/// The base is the `--platform` override (its declared virtual packages are the
/// capabilities) or, when unset, the auto-detected machine (its candidate
/// subdirs and detected virtual packages).
///
/// - base meets the resolution platform -> ok;
/// - base meets only the minimum -> ok, but warn that it runs here by accident;
/// - base is below the minimum -> error.
pub fn verify_run_platform(
    environment: &Environment<'_>,
    target_platform: Option<&PixiPlatformName>,
) -> Result<(), RunPlatformUnsupportedError> {
    let (Some(resolved), Some(minimum)) = environment.installed_platforms() else {
        // No marker (older pixi or not installed) -- nothing to validate.
        return Ok(());
    };

    let (base_subdirs, base_capabilities, base_name) = match target_platform {
        // Explicit `--platform`: trust the named platform's declared capabilities.
        Some(name) => {
            let Some(platform) = environment.named_or_best_declared_platform(Some(name)) else {
                // Not a platform this environment lists; the caller reported it.
                return Ok(());
            };
            (
                vec![platform.subdir()],
                platform.declared_virtual_packages().to_vec(),
                name.clone(),
            )
        }
        // Auto-detected machine: its real virtual packages, and the subdirs it
        // can run (current subdir plus architecture fallbacks).
        None => {
            let current = environment
                .workspace()
                .host_platform(
                    PlatformSource::Defaults,
                    PlatformOverrides::EnvironmentVariableOverrides,
                )
                .subdir();
            let subdirs = environment
                .workspace_manifest()
                .workspace
                .candidate_subdirs(current);
            (
                subdirs,
                environment
                    .workspace()
                    .host_platform(
                        PlatformSource::AutoDetected,
                        PlatformOverrides::EnvironmentVariableOverrides,
                    )
                    .declared_virtual_packages()
                    .to_vec(),
                PixiPlatformName::from(current),
            )
        }
    };

    match classify_run_platform(&base_subdirs, &base_capabilities, &resolved, &minimum) {
        RunPlatformVerdict::Compatible => Ok(()),
        RunPlatformVerdict::OnlyMinimum => {
            warn_runs_by_accident(environment, &base_name);
            Ok(())
        }
        RunPlatformVerdict::BelowMinimum(unmet) => Err(RunPlatformUnsupportedError {
            environment: environment.name().clone(),
            platform: base_name,
            unmet,
        }),
    }
}

/// Whether the current machine can run an environment by design (it
/// satisfies the platform the environment was resolved for) or by accident
/// (only the resolved packages' minimum requirements).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentRunnability {
    /// The machine satisfies the platform the environment was resolved for.
    ByDesign,
    /// The machine only satisfies the resolved packages' minimum requirements.
    ByAccident,
    /// The machine cannot run the environment.
    Unsupported,
}

/// Classify how the current machine (including virtual-package overrides)
/// runs `environment`: from the resolved/minimum platforms recorded in
/// `conda-meta/pixi` when installed, else from the declared platforms and
/// the lock file's minimal requirements.
pub fn classify_environment_runnability(
    environment: &Environment<'_>,
    lock_file: Option<&LockFile>,
) -> EnvironmentRunnability {
    // Mirror `verify_current_platform_can_run_environment`: an explicit
    // platform override means the user vouches for the machine.
    if std::env::var(pixi_consts::consts::PIXI_OVERRIDE_PLATFORM).is_ok() {
        return EnvironmentRunnability::ByDesign;
    }

    if let (Some(resolved), Some(minimum)) = environment.installed_platforms() {
        let current = environment
            .workspace()
            .host_platform(
                PlatformSource::Defaults,
                PlatformOverrides::EnvironmentVariableOverrides,
            )
            .subdir();
        let base_subdirs = environment
            .workspace_manifest()
            .workspace
            .candidate_subdirs(current);
        let base_capabilities = environment
            .workspace()
            .host_platform(
                PlatformSource::AutoDetected,
                PlatformOverrides::EnvironmentVariableOverrides,
            )
            .declared_virtual_packages()
            .to_vec();
        return match classify_run_platform(&base_subdirs, &base_capabilities, &resolved, &minimum) {
            RunPlatformVerdict::Compatible => EnvironmentRunnability::ByDesign,
            RunPlatformVerdict::OnlyMinimum => EnvironmentRunnability::ByAccident,
            RunPlatformVerdict::BelowMinimum(_) => EnvironmentRunnability::Unsupported,
        };
    }

    if environment.best_declared_platform().is_some() {
        return EnvironmentRunnability::ByDesign;
    }
    match lock_file.map(|lock| minimum_compatible_declared_platform(environment, lock)) {
        Some(Ok(_)) => EnvironmentRunnability::ByAccident,
        Some(Err(_)) | None => EnvironmentRunnability::Unsupported,
    }
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

    /// Lock-fallback classification: a machine matching no declared platform
    /// runs the environment "by accident" when the lock-resolved minimum is
    /// satisfied, and not at all when it isn't.
    #[test]
    fn classify_runnability_falls_back_to_lock_minimum() {
        let current = Platform::current();
        let manifest = format!(
            r#"
            [workspace]
            name = "demo"
            channels = []
            platforms = [{{ name = "gpu", platform = "{current}", cuda = "99" }}]
            "#
        );
        let workspace =
            crate::Workspace::from_str(std::path::Path::new("pixi.toml"), &manifest).unwrap();
        let environment = workspace.default_environment();

        let lock = |depends: &str| {
            let source = format!(
                r#"version: 7
platforms:
- name: gpu
  subdir: {current}
  virtual-packages:
  - __cuda=99
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages:
      gpu:
      - conda: https://conda.anaconda.org/conda-forge/{current}/foo-1.0-h0.conda
packages:
- conda: https://conda.anaconda.org/conda-forge/{current}/foo-1.0-h0.conda
{depends}"#
            );
            rattler_lock::LockFile::from_str_with_base_directory(&source, None).unwrap()
        };

        // No lock file: nothing to fall back on.
        assert_eq!(
            classify_environment_runnability(&environment, None),
            EnvironmentRunnability::Unsupported,
        );
        // The resolved package needs no virtual packages: runs by accident.
        assert_eq!(
            classify_environment_runnability(&environment, Some(&lock(""))),
            EnvironmentRunnability::ByAccident,
        );
        // The resolved package needs a `__cuda` no machine provides.
        assert_eq!(
            classify_environment_runnability(
                &environment,
                Some(&lock("  depends:\n  - __cuda >=9999\n")),
            ),
            EnvironmentRunnability::Unsupported,
        );
    }

    /// `install_platform`'s fallback (the fix for an unsatisfied-but-unused
    /// system requirement): when no declared platform matches the host, the
    /// environment still resolves to the minimum-compatible platform as long as
    /// the resolved packages need none of the unsatisfied virtual packages.
    #[test]
    fn minimum_compatible_platform_ignores_unused_requirement() {
        let current = Platform::current();
        let manifest = format!(
            r#"
            [workspace]
            name = "demo"
            channels = []
            platforms = [{{ name = "gpu", platform = "{current}", cuda = "99" }}]
            "#
        );
        let workspace =
            crate::Workspace::from_str(std::path::Path::new("pixi.toml"), &manifest).unwrap();
        let environment = workspace.default_environment();

        let lock = |depends: &str| {
            let source = format!(
                r#"version: 7
platforms:
- name: gpu
  subdir: {current}
  virtual-packages:
  - __cuda=99
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages:
      gpu:
      - conda: https://conda.anaconda.org/conda-forge/{current}/foo-1.0-h0.conda
packages:
- conda: https://conda.anaconda.org/conda-forge/{current}/foo-1.0-h0.conda
{depends}"#
            );
            rattler_lock::LockFile::from_str_with_base_directory(&source, None).unwrap()
        };

        // The resolved package needs no virtual packages: fall back to the gpu
        // platform's subdir even though the host lacks `__cuda=99`.
        let platform = minimum_compatible_declared_platform(&environment, &lock(""))
            .expect("falls back to the minimum-compatible platform");
        assert_eq!(platform.subdir(), current);

        // The resolved package needs a `__cuda` no machine provides: no
        // fallback, and the unmet requirement is surfaced.
        let unmet = minimum_compatible_declared_platform(
            &environment,
            &lock("  depends:\n  - __cuda >=9999\n"),
        )
        .expect_err("an unsatisfiable resolved requirement has no fallback");
        assert!(unmet.iter().any(|vp| vp.name.as_normalized() == "__cuda"));
    }

    /// An environment that resolved no packages at all (its subdir is absent
    /// from the lock-minimum map) needs no virtual packages, so it runs even
    /// when the host can't satisfy the declared requirement -- the fix for
    /// tasks in empty environments under an unsatisfiable system requirement.
    #[test]
    fn minimum_compatible_platform_runs_empty_environment() {
        let current = Platform::current();
        let manifest = format!(
            r#"
            [workspace]
            name = "demo"
            channels = []
            platforms = [{{ name = "gpu", platform = "{current}", cuda = "99" }}]
            "#
        );
        let workspace =
            crate::Workspace::from_str(std::path::Path::new("pixi.toml"), &manifest).unwrap();
        let environment = workspace.default_environment();

        let empty_lock = rattler_lock::LockFile::from_str_with_base_directory(
            r#"version: 7
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages: {}
packages: []
"#,
            None,
        )
        .unwrap();

        let platform = minimum_compatible_declared_platform(&environment, &empty_lock)
            .expect("an empty environment runs regardless of the declared requirement");
        assert_eq!(platform.subdir(), current);
    }

    /// A machine-compatible declared platform classifies as "by design"
    /// without consulting any lock file.
    #[test]
    fn classify_runnability_by_design_via_declared_platform() {
        let current = Platform::current();
        let manifest = format!(
            r#"
            [workspace]
            name = "demo"
            channels = []
            platforms = ["{current}"]
            "#
        );
        let workspace =
            crate::Workspace::from_str(std::path::Path::new("pixi.toml"), &manifest).unwrap();
        assert_eq!(
            classify_environment_runnability(&workspace.default_environment(), None),
            EnvironmentRunnability::ByDesign,
        );
    }

    #[test]
    fn declared_cuda_overrides_default() {
        let pp = pixi_manifest::PixiPlatform::new(
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

        // A platform with no declared cuda should not emit a __cuda VP.
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

    fn gvp(name: &str, version: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: rattler_conda_types::PackageName::try_from(name).unwrap(),
            version: rattler_conda_types::Version::from_str(version).unwrap(),
            build_string: String::new(),
        }
    }

    fn platform_data(subdir: Platform, vps: Vec<GenericVirtualPackage>) -> PlatformData {
        PlatformData {
            subdir,
            virtual_packages: vps,
        }
    }

    #[test]
    fn classify_compatible_when_base_meets_resolution() {
        // Base provides cuda 12.4; resolution needs 12.0 and minimum 12.0.
        let resolved = platform_data(Platform::Linux64, vec![gvp("__cuda", "12.0")]);
        let minimum = platform_data(Platform::Linux64, vec![gvp("__cuda", "12.0")]);
        let verdict = classify_run_platform(
            &[Platform::Linux64],
            &[gvp("__cuda", "12.4")],
            &resolved,
            &minimum,
        );
        assert_eq!(verdict, RunPlatformVerdict::Compatible);
    }

    #[test]
    fn classify_only_minimum_when_below_resolution_but_meets_minimum() {
        // Resolution wanted glibc 2.28, the package floor is only 2.17, and the
        // base provides 2.17 -- it runs, but by accident.
        let resolved = platform_data(Platform::Linux64, vec![gvp("__glibc", "2.28")]);
        let minimum = platform_data(Platform::Linux64, vec![gvp("__glibc", "2.17")]);
        let verdict = classify_run_platform(
            &[Platform::Linux64],
            &[gvp("__glibc", "2.17")],
            &resolved,
            &minimum,
        );
        assert_eq!(verdict, RunPlatformVerdict::OnlyMinimum);
    }

    #[test]
    fn classify_below_minimum_reports_unmet() {
        // Base glibc 2.12 is below the 2.17 floor the installed packages need.
        let resolved = platform_data(Platform::Linux64, vec![gvp("__glibc", "2.28")]);
        let minimum = platform_data(Platform::Linux64, vec![gvp("__glibc", "2.17")]);
        let verdict = classify_run_platform(
            &[Platform::Linux64],
            &[gvp("__glibc", "2.12")],
            &resolved,
            &minimum,
        );
        match verdict {
            RunPlatformVerdict::BelowMinimum(unmet) => {
                assert_eq!(unmet.len(), 1);
                assert_eq!(unmet[0].name.as_normalized(), "__glibc");
            }
            other => panic!("expected BelowMinimum, got {other:?}"),
        }
    }

    #[test]
    fn classify_below_minimum_on_subdir_mismatch() {
        // A subdir outside the base's candidates can never satisfy.
        let resolved = platform_data(Platform::Osx64, vec![]);
        let minimum = platform_data(Platform::Osx64, vec![]);
        let verdict = classify_run_platform(&[Platform::Linux64], &[], &resolved, &minimum);
        assert!(matches!(verdict, RunPlatformVerdict::BelowMinimum(_)));
    }

    #[test]
    fn classify_compatible_via_candidate_subdir() {
        // An emulated subdir (osx-64 among an osx-arm64 host's candidates) with
        // satisfied virtual packages is compatible.
        let resolved = platform_data(Platform::Osx64, vec![gvp("__osx", "11.0")]);
        let minimum = platform_data(Platform::Osx64, vec![]);
        let verdict = classify_run_platform(
            &[Platform::OsxArm64, Platform::Osx64],
            &[gvp("__osx", "13.0")],
            &resolved,
            &minimum,
        );
        assert_eq!(verdict, RunPlatformVerdict::Compatible);
    }
}
