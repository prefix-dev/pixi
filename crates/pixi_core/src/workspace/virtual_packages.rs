use crate::environment::{PlatformData, RequiredPlatform};
use crate::lock_file::virtual_packages::{
    MachineValidationError, compute_required_virtual_package_specs, unmet_requirements,
    validate_system_meets_environment_requirements,
};
use crate::workspace::{
    Environment, HasWorkspaceRef,
    errors::{UnsupportedPlatformError, format_specs},
};
use fancy_display::FancyDisplay;
use miette::Diagnostic;
use pixi_manifest::platform::host::{host_capabilities, host_subdir};
use pixi_manifest::{
    EnvironmentName, FeaturesExt, HasWorkspaceManifest, PixiPlatform, PixiPlatformName,
    platform::{candidate_subdirs, solver_virtual_packages, unsatisfied_capabilities},
};
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, Platform};
use rattler_lock::LockFile;
use rattler_virtual_packages::VirtualPackage;
use std::collections::HashSet;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use thiserror::Error;

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
///    fall back to the virtual-package requirements the resolved dependencies
///    actually place on the machine (a declared platform may promise virtual
///    packages they don't need). If the machine meets those, the environment can
///    run.
///
/// Outcomes: (1) holds -> ok; (1) fails but (2) holds -> ok with a warning;
/// both fail -> error listing the unmet requirements.
pub fn verify_current_platform_can_run_environment(
    environment: &Environment<'_>,
    lock_file: Option<&LockFile>,
) -> Result<(), VerifyCurrentPlatformError> {
    // When overriding platform skip validation entirely.
    // The host platform wouldn't satisfy the requirements
    if std::env::var(pixi_consts::consts::PIXI_OVERRIDE_PLATFORM).is_ok() {
        return Ok(());
    }

    // Check 1:
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

    let Some(lock_file) = lock_file else {
        return Err(VerifyCurrentPlatformError::from(Box::new(
            environment.unsupported_platform_error(),
        )));
    };

    // Check 2:
    match minimum_compatible_declared_platform(environment, lock_file) {
        Ok(_) => {
            // Check 1 failed but the resolution is compatible -- continue.
            tracing::warn!(
                "The current machine is not one of the platforms declared for environment '{}', but the resolved dependencies are compatible with it, continuing.",
                environment.name().fancy_display(),
            );
            Ok(())
        }
        // Both checks failed:
        Err(unmet) => {
            let mut error = environment.unsupported_platform_error();
            error.unmet_requirements = unmet;
            Err(VerifyCurrentPlatformError::from(Box::new(error)))
        }
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
) -> Result<&'p PixiPlatform, Vec<MatchSpec>> {
    let current = host_subdir();
    let system_virtual_packages = host_capabilities();
    let candidate_subdirs = candidate_subdirs(current);

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
    let required =
        compute_required_virtual_package_specs(lock_file, environment.name(), &declared_platforms);

    let mut unmet: Option<Vec<MatchSpec>> = None;
    for subdir in &candidate_subdirs {
        // A subdir with no resolved packages requires no virtual packages, so
        // the machine trivially satisfies it -- e.g. an empty environment whose
        // only content is tasks still runs under an unsatisfiable requirement.
        let unsatisfied = required
            .get(subdir)
            .map(|specs| unmet_requirements(specs, &system_virtual_packages))
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

/// Why the platform the environment was installed for cannot run here.
#[derive(Debug)]
enum RunPlatformFailure {
    /// The machine can run the subdir, but does not meet these requirements the
    /// installed packages place on it.
    UnmetRequirements(Vec<MatchSpec>),
    /// The environment was installed for a subdir, which the machine cannot run.
    UnrunnableSubdir(Platform),
}

/// The platform the environment was installed for cannot run the installed
/// packages.
#[derive(Debug, Error)]
#[error("the installed environment '{environment}' cannot run on platform '{platform}'")]
pub struct RunPlatformUnsupportedError {
    environment: EnvironmentName,
    platform: PixiPlatformName,
    /// The subdir behind `platform`. Kept alongside the name because a
    /// `CONDA_OVERRIDE_*` suggestion is only useful on the platforms that honor it
    subdir: Platform,
    failure: RunPlatformFailure,
}

impl Diagnostic for RunPlatformUnsupportedError {
    /// Generate a good help text for the `RunPlatformUnsupportedError`
    fn help(&self) -> Option<Box<dyn Display + '_>> {
        match &self.failure {
            RunPlatformFailure::UnrunnableSubdir(subdir) => Some(Box::new(format!(
                "It was installed for subdir '{subdir}', which this machine cannot run. \
                 Reinstall for this machine with 'pixi install'."
            ))),
            RunPlatformFailure::UnmetRequirements(unmet) => {
                let requirements = format_specs(unmet);
                let base = format!(
                    "The installed packages require virtual packages this platform does not \
                     provide: [{requirements}]. Reinstall for this machine with 'pixi install', \
                     or select a compatible platform with '--platform'."
                );
                let overrides = crate::workspace::errors::spec_override_hints(unmet, self.subdir);
                Some(Box::new(if overrides.is_empty() {
                    base
                } else {
                    format!(
                        "{base}\nOr mock them via the environment, e.g.:\n  {}",
                        overrides.join("\n  ")
                    )
                }))
            }
        }
    }
}

/// How a base platform compares to the resolved/minimum platforms an
/// environment was installed for.
#[derive(Debug, PartialEq, Eq)]
enum RunPlatformVerdict {
    /// The base meets the resolution platform: it runs as intended.
    Compatible,
    /// The base meets only the minimum requirements, not the full resolution
    /// platform: it runs, but the environment was resolved for more. Carries
    /// the resolution platform's virtual packages the base fails to provide
    /// (empty when only the resolution subdir is out of reach).
    OnlyMinimum(Vec<GenericVirtualPackage>),
    /// The base is below the minimum: the installed packages cannot run, with
    /// the requirements it fails to meet.
    BelowMinimum(Vec<MatchSpec>),
    /// The environment was installed for a subdir the base cannot run at all.
    UnrunnableSubdir(Platform),
}

/// Classify a base platform against the resolution platform and the
/// requirements an environment was installed with.
fn classify_run_platform(
    base_subdirs: &[Platform],
    base_capabilities: &[GenericVirtualPackage],
    resolved: &PlatformData,
    minimum: &RequiredPlatform,
) -> RunPlatformVerdict {
    let unmet_resolved = unsatisfied_capabilities(resolved.virtual_packages(), base_capabilities);
    let unmet_minimum = unmet_requirements(minimum.requirements(), base_capabilities);
    let meets_resolved = base_subdirs.contains(&resolved.subdir()) && unmet_resolved.is_empty();
    let meets_minimum = base_subdirs.contains(&minimum.subdir()) && unmet_minimum.is_empty();

    if meets_resolved {
        RunPlatformVerdict::Compatible
    } else if meets_minimum {
        RunPlatformVerdict::OnlyMinimum(unmet_resolved)
    } else if base_subdirs.contains(&minimum.subdir()) {
        RunPlatformVerdict::BelowMinimum(unmet_minimum)
    } else {
        RunPlatformVerdict::UnrunnableSubdir(minimum.subdir())
    }
}

/// The body of the "runs by accident" warning: which resolution requirement
/// the base fails, why the environment still runs, and the re-resolve risk.
/// `unresolvable_subdir` is the resolution subdir when the base cannot run it
/// at all; `unmet` are the resolution platform's virtual packages the base
/// fails to provide; `machine` are the base's capabilities, used to
/// distinguish a missing virtual package from one at a too-low version.
fn describe_resolution_gap(
    base: &PixiPlatformName,
    unresolvable_subdir: Option<Platform>,
    unmet: &[GenericVirtualPackage],
    machine: &[GenericVirtualPackage],
) -> String {
    const RERESOLVE: &str = "the next re-resolve (e.g. 'pixi update' or a change to the manifest)";

    if let Some(subdir) = unresolvable_subdir {
        return format!(
            "was resolved for platform '{subdir}', which this machine cannot run. \
             The currently installed packages are compatible with '{base}', so the \
             environment still runs, but {RERESOLVE} may install packages that \
             only run on '{subdir}'."
        );
    }

    if unmet.is_empty() {
        // Unreachable in practice: `OnlyMinimum` implies a subdir or
        // virtual-package gap. Keep a generic message rather than panic.
        return format!(
            "was resolved for a richer platform than '{base}' provides; this machine \
             only meets the installed packages' minimum requirements."
        );
    }

    let requirements = unmet
        .iter()
        .map(describe_requirement)
        .collect::<Vec<_>>()
        .join(", ");
    let provided = unmet
        .iter()
        .map(
            |required| match machine.iter().find(|sys| sys.name == required.name) {
                Some(sys) => format!("only provides '{}'", describe_provided(sys)),
                None => format!("does not provide '{}'", required.name.as_normalized()),
            },
        )
        .collect::<Vec<_>>()
        .join(" and ");
    let pronoun = if unmet.len() == 1 { "it" } else { "them" };
    format!(
        "requires {requirements}, but this machine {provided}. The currently installed \
         packages don't actually need {pronoun}, so the environment still runs, but \
         {RERESOLVE} may install packages that do, which would no longer run on this \
         machine."
    )
}

/// A required virtual package, as the thing the user has to satisfy.
///
/// `__archspec` carries a constant version and names its microarchitecture in
/// the build string, so rendering it like the others produces the useless
/// "archspec >=0"; name the microarchitecture instead.
fn describe_requirement(required: &GenericVirtualPackage) -> String {
    let name = required.name.as_normalized().trim_start_matches('_');
    match archspec_microarchitecture_of(required) {
        Some(microarchitecture) => format!("{name} {microarchitecture}"),
        None => format!("{name} >={}", required.version),
    }
}

/// What the machine offers for a required virtual package, in the same terms.
fn describe_provided(provided: &GenericVirtualPackage) -> String {
    let name = provided.name.as_normalized();
    match archspec_microarchitecture_of(provided) {
        Some(microarchitecture) => format!("{name} {microarchitecture}"),
        None => format!("{name} {}", provided.version),
    }
}

/// The microarchitecture `package` names, if it is an `__archspec` that names
/// one at all.
fn archspec_microarchitecture_of(package: &GenericVirtualPackage) -> Option<&str> {
    (package.name.as_normalized() == "__archspec")
        .then(|| pixi_manifest::platform::archspec_microarchitecture(&package.build_string))
        .flatten()
}

/// Marker-file paths we've already emitted the "runs by accident" warning for
/// in this process, so a multi-task run warns at most once per environment.
static BY_ACCIDENT_WARNED: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Warn -- at most once per process and once ever per workspace -- that
/// `environment` runs only because its minimum requirements happen to be met,
/// not the platform it was resolved for. `gap` is the message body from
/// [`describe_resolution_gap`]. Mirrors the persisted one-time-message scheme
/// used by [`Environment::emit_emulation_warning`].
fn warn_runs_by_accident(environment: &Environment<'_>, gap: &str) {
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

    tracing::warn!("Environment '{}' {gap}", environment.name().fancy_display());

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
/// - base meets only the minimum -> ok, but warn which resolution requirement
///   the base misses (it runs here by accident);
/// - base is below the minimum -> error.
pub fn verify_run_platform(
    environment: &Environment<'_>,
    target_platform: Option<&PixiPlatformName>,
) -> Result<(), RunPlatformUnsupportedError> {
    // An explicit platform override means the user vouches for the machine, so
    // host validation is skipped.
    if std::env::var(pixi_consts::consts::PIXI_OVERRIDE_PLATFORM).is_ok() {
        return Ok(());
    }

    let (Some(resolved), Some(minimum)) = environment.installed_platforms() else {
        // No marker (older pixi or not installed) -- nothing to validate.
        return Ok(());
    };

    let (base_subdirs, base_capabilities, base_name, base_subdir) = match target_platform {
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
                platform.subdir(),
            )
        }
        // Auto-detected machine: its real virtual packages, and the subdirs it
        // can run (current subdir plus architecture fallbacks).
        None => {
            let current = host_subdir();
            let subdirs = candidate_subdirs(current);
            (
                subdirs,
                host_capabilities(),
                PixiPlatformName::from(current),
                current,
            )
        }
    };

    match classify_run_platform(&base_subdirs, &base_capabilities, &resolved, &minimum) {
        RunPlatformVerdict::Compatible => Ok(()),
        RunPlatformVerdict::OnlyMinimum(unmet) => {
            let unresolvable_subdir =
                (!base_subdirs.contains(&resolved.subdir())).then_some(resolved.subdir());
            let gap = describe_resolution_gap(
                &base_name,
                unresolvable_subdir,
                &unmet,
                &base_capabilities,
            );
            warn_runs_by_accident(environment, &gap);
            Ok(())
        }
        RunPlatformVerdict::BelowMinimum(unmet) => Err(RunPlatformUnsupportedError {
            environment: environment.name().clone(),
            platform: base_name,
            subdir: base_subdir,
            failure: RunPlatformFailure::UnmetRequirements(unmet),
        }),
        RunPlatformVerdict::UnrunnableSubdir(subdir) => Err(RunPlatformUnsupportedError {
            environment: environment.name().clone(),
            platform: base_name,
            subdir: base_subdir,
            failure: RunPlatformFailure::UnrunnableSubdir(subdir),
        }),
    }
}

/// Whether the current machine can run an environment by design (it
/// satisfies the platform the environment was resolved for) or by accident
/// (only the resolved packages' minimum requirements).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentRunnability {
    /// The environment has no dependencies, so it installs nothing that could
    /// require a virtual package the machine lacks.
    NoDependencies,
    /// The machine satisfies the platform the environment was resolved for.
    ByDesign,
    /// The machine only satisfies the resolved packages' minimum requirements.
    ByAccident,
    /// The machine cannot run the environment.
    Unsupported,
}

/// Whether `environment` declares any conda or PyPI dependency on any of its
/// platforms. An environment with no dependencies installs nothing, so no
/// package can require a virtual package the machine lacks.
pub(crate) fn environment_has_dependencies(environment: &Environment<'_>) -> bool {
    let has_conda_dependencies = |platform: Option<&PixiPlatform>| {
        !environment.combined_dependencies(platform).is_empty()
            || !environment.combined_dev_dependencies(platform).is_empty()
    };

    if environment.has_pypi_dependencies() {
        return true;
    }
    if has_conda_dependencies(None) {
        return true;
    }
    // Passing `None` skips platform-specific tables, so check each declared
    // platform for target-specific dependencies too.
    let manifest = environment.workspace_manifest();
    let env_platform_names = environment.platforms();
    manifest
        .workspace
        .platforms
        .iter()
        .filter(|platform| env_platform_names.contains(platform.name()))
        .any(|platform| has_conda_dependencies(Some(platform)))
}

/// Classify how the current machine (including virtual-package overrides)
/// runs `environment`:
///
/// - it runs by design when the machine satisfies a declared platform's virtual
///   packages (the platform it resolves for), so its prefix builds normally even
///   when it has no dependencies;
/// - otherwise an environment without dependencies installs nothing that could
///   require a virtual package the machine lacks, so platform requirements don't
///   apply;
/// - by accident when the machine only meets the minimum the resolved packages
///   require (computed from the lock file);
/// - and is unsupported when it meets neither.
///
/// Unlike run-time validation, this never consults the `conda-meta/pixi`
/// marker: the marker records the platform a *previous* install resolved for,
/// which goes stale when the manifest changes.
pub fn classify_environment_runnability(
    environment: &Environment<'_>,
    lock_file: Option<&LockFile>,
) -> EnvironmentRunnability {
    // Mirror `verify_current_platform_can_run_environment`: an explicit
    // platform override means the user vouches for the machine.
    if std::env::var(pixi_consts::consts::PIXI_OVERRIDE_PLATFORM).is_ok() {
        return EnvironmentRunnability::ByDesign;
    }

    // A machine that satisfies a declared platform runs the environment as
    // resolved, so build its prefix normally -- even without dependencies, an
    // empty prefix still backs activation env vars.
    if environment.best_declared_platform().is_some() {
        return EnvironmentRunnability::ByDesign;
    }

    if !environment_has_dependencies(environment) {
        return EnvironmentRunnability::NoDependencies;
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
        solver_virtual_packages(platform)
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
            let packages = solver_virtual_packages(&pp)
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

            [dependencies]
            foo = "*"
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
        // Reported as the spec the package carries, not as a bare version.
        assert_eq!(
            unmet.iter().map(ToString::to_string).collect::<Vec<_>>(),
            vec!["__cuda >=9999"]
        );
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

            [dependencies]
            foo = "*"
            "#
        );
        let workspace =
            crate::Workspace::from_str(std::path::Path::new("pixi.toml"), &manifest).unwrap();
        assert_eq!(
            classify_environment_runnability(&workspace.default_environment(), None),
            EnvironmentRunnability::ByDesign,
        );
    }

    /// An environment without dependencies classifies as "no dependencies"
    /// even when its declared platform demands virtual packages this machine
    /// lacks: nothing it installs can require them.
    #[test]
    fn classify_runnability_no_dependencies() {
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
        assert_eq!(
            classify_environment_runnability(&workspace.default_environment(), None),
            EnvironmentRunnability::NoDependencies,
        );
    }

    /// Develop dependencies bring their own transitive packages into the
    /// prefix, so an environment declaring only those still has dependencies.
    #[test]
    fn classify_runnability_counts_dev_dependencies() {
        let current = Platform::current();
        let manifest = format!(
            r#"
            [workspace]
            name = "demo"
            channels = []
            platforms = [{{ name = "gpu", platform = "{current}", cuda = "99" }}]
            preview = ["pixi-build"]

            [dev]
            mypkg = {{ path = "mypkg" }}
            "#
        );
        let workspace =
            crate::Workspace::from_str(std::path::Path::new("pixi.toml"), &manifest).unwrap();
        assert_eq!(
            classify_environment_runnability(&workspace.default_environment(), None),
            EnvironmentRunnability::Unsupported,
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
        let packages = solver_virtual_packages(&pp);
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
            !solver_virtual_packages(&bare)
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
        let libc = solver_virtual_packages(&pp)
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

    /// The requirement side: match specs as a resolved package's `depends`
    /// spells them, not concrete virtual packages.
    fn required_platform(subdir: Platform, specs: &[&str]) -> RequiredPlatform {
        RequiredPlatform::new(
            subdir,
            specs
                .iter()
                .map(|raw| {
                    MatchSpec::from_str(raw, rattler_conda_types::ParseStrictness::Lenient).unwrap()
                })
                .collect(),
        )
    }

    #[test]
    fn classify_compatible_when_base_meets_resolution() {
        // Base provides cuda 12.4; resolution needs 12.0 and minimum 12.0.
        let resolved = platform_data(Platform::Linux64, vec![gvp("__cuda", "12.0")]);
        let minimum = required_platform(Platform::Linux64, &["__cuda >=12.0"]);
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
        // base provides 2.17 -- it runs, but by accident. The verdict carries
        // the resolution requirement the base misses.
        let resolved = platform_data(Platform::Linux64, vec![gvp("__glibc", "2.28")]);
        let minimum = required_platform(Platform::Linux64, &["__glibc >=2.17"]);
        let verdict = classify_run_platform(
            &[Platform::Linux64],
            &[gvp("__glibc", "2.17")],
            &resolved,
            &minimum,
        );
        assert_eq!(
            verdict,
            RunPlatformVerdict::OnlyMinimum(vec![gvp("__glibc", "2.28")])
        );
    }

    #[test]
    fn resolution_gap_names_missing_virtual_package() {
        let base = PixiPlatformName::from(Platform::Linux64);
        let gap = describe_resolution_gap(&base, None, &[gvp("__cuda", "12")], &[]);
        insta::assert_snapshot!(gap, @"requires cuda >=12, but this machine does not provide '__cuda'. The currently installed packages don't actually need it, so the environment still runs, but the next re-resolve (e.g. 'pixi update' or a change to the manifest) may install packages that do, which would no longer run on this machine.");
    }

    #[test]
    fn resolution_gap_names_too_low_virtual_package() {
        let base = PixiPlatformName::from(Platform::Linux64);
        let gap = describe_resolution_gap(
            &base,
            None,
            &[gvp("__glibc", "2.28")],
            &[gvp("__glibc", "2.17")],
        );
        insta::assert_snapshot!(gap, @"requires glibc >=2.28, but this machine only provides '__glibc 2.17'. The currently installed packages don't actually need it, so the environment still runs, but the next re-resolve (e.g. 'pixi update' or a change to the manifest) may install packages that do, which would no longer run on this machine.");
    }

    #[test]
    fn resolution_gap_joins_multiple_requirements() {
        let base = PixiPlatformName::from(Platform::Linux64);
        let gap = describe_resolution_gap(
            &base,
            None,
            &[gvp("__cuda", "12"), gvp("__glibc", "2.28")],
            &[gvp("__glibc", "2.17")],
        );
        insta::assert_snapshot!(gap, @"requires cuda >=12, glibc >=2.28, but this machine does not provide '__cuda' and only provides '__glibc 2.17'. The currently installed packages don't actually need them, so the environment still runs, but the next re-resolve (e.g. 'pixi update' or a change to the manifest) may install packages that do, which would no longer run on this machine.");
    }

    #[test]
    fn resolution_gap_reports_unrunnable_subdir() {
        let base = PixiPlatformName::from(Platform::Linux64);
        let gap = describe_resolution_gap(&base, Some(Platform::LinuxAarch64), &[], &[]);
        insta::assert_snapshot!(gap, @"was resolved for platform 'linux-aarch64', which this machine cannot run. The currently installed packages are compatible with 'linux-64', so the environment still runs, but the next re-resolve (e.g. 'pixi update' or a change to the manifest) may install packages that only run on 'linux-aarch64'.");
    }

    #[test]
    fn classify_below_minimum_reports_unmet() {
        // Base glibc 2.12 is below the 2.17 floor the installed packages need.
        let resolved = platform_data(Platform::Linux64, vec![gvp("__glibc", "2.28")]);
        let minimum = required_platform(Platform::Linux64, &["__glibc >=2.17"]);
        let verdict = classify_run_platform(
            &[Platform::Linux64],
            &[gvp("__glibc", "2.12")],
            &resolved,
            &minimum,
        );
        match verdict {
            RunPlatformVerdict::BelowMinimum(unmet) => {
                assert_eq!(
                    unmet.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    vec!["__glibc >=2.17"]
                );
            }
            other => panic!("expected BelowMinimum, got {other:?}"),
        }
    }

    #[test]
    fn run_platform_refusal_suggests_overrides_from_the_requirements() {
        let spec = |raw: &str| {
            MatchSpec::from_str(raw, rattler_conda_types::ParseStrictness::Lenient).unwrap()
        };
        let error = RunPlatformUnsupportedError {
            environment: EnvironmentName::Default,
            platform: PixiPlatformName::from(Platform::Linux64),
            subdir: Platform::Linux64,
            failure: RunPlatformFailure::UnmetRequirements(vec![
                spec("__glibc >=2.28"),
                spec("__cuda >=12"),
                spec("__unix"),
            ]),
        };
        let help = error
            .help()
            .expect("a refusal always explains itself")
            .to_string();

        // The requirements themselves, as written.
        assert!(
            help.contains("[__glibc >=2.28, __cuda >=12, __unix]"),
            "{help}"
        );
        // The remedies that actually fix the environment come first.
        assert!(help.contains("pixi install"), "{help}");
        // A value drawn from each requirement that has an override.
        assert!(help.contains("CONDA_OVERRIDE_GLIBC=2.28"), "{help}");
        assert!(help.contains("CONDA_OVERRIDE_CUDA=12"), "{help}");
        // `__unix` has no override, so it is named but not suggested.
        assert!(!help.contains("CONDA_OVERRIDE_UNIX"), "{help}");
    }

    #[test]
    fn run_platform_refusal_omits_the_override_section_when_useless() {
        let error = RunPlatformUnsupportedError {
            environment: EnvironmentName::Default,
            platform: PixiPlatformName::from(Platform::Linux64),
            subdir: Platform::Linux64,
            failure: RunPlatformFailure::UnmetRequirements(vec![
                MatchSpec::from_str("__unix", rattler_conda_types::ParseStrictness::Lenient)
                    .unwrap(),
            ]),
        };
        let help = error
            .help()
            .expect("a refusal always explains itself")
            .to_string();
        assert!(!help.contains("e.g."), "{help}");
        assert!(help.contains("pixi install"), "{help}");
    }

    #[test]
    fn run_platform_refusal_omits_overrides_the_host_platform_ignores() {
        // Requirements whose override this host ignores, per CEP 30.
        let ignored_here: &[(&str, &str)] = if cfg!(target_os = "windows") {
            &[
                ("__osx", "CONDA_OVERRIDE_OSX"),
                ("__linux", "CONDA_OVERRIDE_LINUX"),
            ]
        } else if cfg!(target_os = "macos") {
            &[
                ("__win", "CONDA_OVERRIDE_WIN"),
                ("__linux", "CONDA_OVERRIDE_LINUX"),
            ]
        } else {
            &[
                ("__win", "CONDA_OVERRIDE_WIN"),
                ("__osx", "CONDA_OVERRIDE_OSX"),
            ]
        };

        for (name, env_var) in ignored_here {
            let error = RunPlatformUnsupportedError {
                environment: EnvironmentName::Default,
                platform: PixiPlatformName::from(Platform::current()),
                subdir: Platform::current(),
                failure: RunPlatformFailure::UnmetRequirements(vec![
                    MatchSpec::from_str(name, rattler_conda_types::ParseStrictness::Lenient)
                        .unwrap(),
                ]),
            };
            let help = error
                .help()
                .expect("a refusal always explains itself")
                .to_string();
            assert!(
                !help.contains(env_var),
                "{env_var} does nothing on this host, so suggesting it is a dead end:\n{help}"
            );
        }
    }

    #[test]
    fn classify_reports_an_unrunnable_subdir_rather_than_its_requirements() {
        let resolved = platform_data(Platform::Osx64, vec![]);
        let minimum = required_platform(Platform::Osx64, &["__osx >=11.0"]);
        let verdict = classify_run_platform(
            &[Platform::Linux64],
            &[gvp("__osx", "13.0")],
            &resolved,
            &minimum,
        );
        assert_eq!(
            verdict,
            RunPlatformVerdict::UnrunnableSubdir(Platform::Osx64)
        );
    }

    #[test]
    fn run_platform_refusal_for_a_subdir_mismatch_names_the_subdir() {
        let error = RunPlatformUnsupportedError {
            environment: EnvironmentName::Default,
            platform: PixiPlatformName::from(Platform::Linux64),
            subdir: Platform::Linux64,
            failure: RunPlatformFailure::UnrunnableSubdir(Platform::Osx64),
        };
        let help = error
            .help()
            .expect("a refusal always explains itself")
            .to_string();
        assert!(help.contains("installed for subdir 'osx-64'"), "{help}");
        assert!(help.contains("pixi install"), "{help}");
        // No virtual package is named, and no remedy that cannot work.
        assert!(!help.contains("virtual packages"), "{help}");
        assert!(!help.contains("CONDA_OVERRIDE"), "{help}");
        assert!(!help.contains("--platform"), "{help}");
    }

    #[test]
    fn classify_compatible_via_candidate_subdir() {
        // An emulated subdir (osx-64 among an osx-arm64 host's candidates) with
        // satisfied virtual packages is compatible.
        let resolved = platform_data(Platform::Osx64, vec![gvp("__osx", "11.0")]);
        let minimum = required_platform(Platform::Osx64, &[]);
        let verdict = classify_run_platform(
            &[Platform::OsxArm64, Platform::Osx64],
            &[gvp("__osx", "13.0")],
            &resolved,
            &minimum,
        );
        assert_eq!(verdict, RunPlatformVerdict::Compatible);
    }
}
