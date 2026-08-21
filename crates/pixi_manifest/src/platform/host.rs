//! What this machine looks like as a [`PixiPlatform`].
//!
//! Two questions about the local machine that must not be confused:
//!
//! * *the platform we target* -- [`host_subdir`], which honors
//!   `PIXI_OVERRIDE_PLATFORM`. Everything that selects, solves, or installs an
//!   environment goes through it.
//! * *the machine we execute on* -- `Platform::current()`, which is right only
//!   where pixi is about to run or build something locally (source builds
//!   cannot cross-compile, and the build backends run on the real host).
//!
//! Two platform constructors sit on top of the subdir: [`host_baseline`]
//! describes it the way pixi assumes it, [`detect_host`] describes the machine
//! the way it is. Both apply `CONDA_OVERRIDE_*`.

use rattler_conda_types::{GenericVirtualPackage, Platform, Version};
use rattler_virtual_packages::{
    Archspec, Cuda, CudaArch, DetectVirtualPackageError, EnvOverride, LibC, Linux, Osx, Override,
    VirtualPackageOverrides, VirtualPackages, Windows,
};

use super::{
    PixiPlatform, PixiPlatformError, candidate_subdirs, is_subdir_default,
    subdir_default_virtual_packages,
};

/// A host platform could not be determined.
#[derive(Debug, thiserror::Error)]
pub enum HostDetectionError {
    #[error("failed to detect the virtual packages of this machine")]
    Detect(#[from] DetectVirtualPackageError),

    #[error(transparent)]
    Platform(#[from] PixiPlatformError),
}

/// The subdir pixi treats as this machine's, honoring `PIXI_OVERRIDE_PLATFORM`.
///
/// This is *the platform we target*. Use it everywhere pixi selects, solves,
/// or installs an environment; reach for `Platform::current()` only where pixi
/// is about to run or build something on this machine for real.
///
/// Only `PIXI_OVERRIDE_PLATFORM` is read here, so an invalid `CONDA_OVERRIDE_*`
/// goes unwarned until the virtual packages are built.
pub fn host_subdir() -> Platform {
    std::env::var(pixi_consts::consts::PIXI_OVERRIDE_PLATFORM)
        .ok()
        .and_then(|value| match value.parse::<Platform>() {
            Ok(platform) => Some(platform),
            Err(_) => {
                tracing::warn!("Invalid value for PIXI_OVERRIDE_PLATFORM='{value}', ignoring.");
                None
            }
        })
        .unwrap_or_else(Platform::current)
}

/// What a `CONDA_OVERRIDE_*` variable says about the virtual package it
/// governs.
#[derive(Clone)]
enum VersionOverride {
    /// Unset, unusable, or a package no variable governs: leave the detected
    /// package as it is.
    Untouched,

    /// Set empty: this machine does not have the package at all.
    Disabled,

    /// Set to a version: use it, and add the package if nothing detected it.
    Pinned(Version),
}

impl VersionOverride {
    /// The version this override introduces, if it introduces one.
    fn pinned(&self) -> Option<Version> {
        match self {
            VersionOverride::Pinned(version) => Some(version.clone()),
            VersionOverride::Untouched | VersionOverride::Disabled => None,
        }
    }
}

/// Read the `CONDA_OVERRIDE_*` variable that governs `T`.
///
/// The variable name and the parsing rules both hang off the type, so `T` is
/// what selects the slot. `to_version` is a parameter only because the version
/// field is not part of the trait: `Windows` carries an optional one where the
/// rest carry a plain [`Version`].
///
/// An unusable value warns rather than being dropped on the floor: pixi applies
/// these itself, so nothing else would tell the user their override was
/// ignored.
fn version_override<T: EnvOverride>(to_version: impl Fn(T) -> Version) -> VersionOverride {
    if std::env::var_os(T::DEFAULT_ENV_NAME).is_none() {
        return VersionOverride::Untouched;
    }
    match T::detect_with_fallback(&Override::DefaultEnvVar, || Ok(None)) {
        Ok(Some(value)) => VersionOverride::Pinned(to_version(value)),
        Ok(None) => VersionOverride::Disabled,
        Err(error) => {
            tracing::warn!("Ignoring {}: {error}", T::DEFAULT_ENV_NAME);
            VersionOverride::Untouched
        }
    }
}

/// Whether `subdir` can carry the OS-specific virtual package `name` at all.
/// Mirrors the per-platform gating in rattler's `detect_for_platform`; every
/// other slot (`__cuda`, `__archspec`, ...) is valid on any subdir.
fn carried_by_subdir(name: &str, subdir: Platform) -> bool {
    match name {
        "__osx" => subdir.is_osx(),
        "__win" => subdir.is_windows(),
        "__linux" | "__glibc" | "__musl" | "__eglibc" => subdir.is_linux(),
        _ => true,
    }
}

/// Apply `CONDA_OVERRIDE_*` to `packages`, with rattler's semantics: unset
/// keeps the detected version, a value replaces it (adding the package if
/// nothing detected it), and an empty value removes the package entirely.
///
/// Detect with [`VirtualPackageOverrides::default`] and apply this rather than
/// detecting with [`VirtualPackageOverrides::from_env`]: rattler fails the
/// whole detection on a single unparsable value, where this validates per slot
/// and warns.
///
/// `subdir` is the platform being described, and an override only introduces a
/// package that subdir can carry: `CONDA_OVERRIDE_OSX` does not put `__osx` on
/// a win-64 target, the same way rattler's `detect_for_platform` filters.
pub fn apply_conda_overrides(packages: &mut Vec<GenericVirtualPackage>, subdir: Platform) {
    // Read each variable once, so a bad value is reported once rather than
    // per pass below.
    let cuda = version_override::<Cuda>(|cuda| cuda.version);
    let linux = version_override::<Linux>(|linux| linux.version);
    let osx = version_override::<Osx>(|osx| osx.version);
    let cuda_arch = version_override::<CudaArch>(|arch| arch.version);
    // `Windows::parse_version` always fills the version in, so the fallback
    // only covers the unreachable `None` arm of rattler's optional field.
    let win = version_override::<Windows>(|win| win.version.unwrap_or_else(|| Version::major(0)));

    packages.retain_mut(|package| {
        let outcome = match package.name.as_normalized() {
            "__cuda" => cuda.clone(),
            "__cuda_arch" => cuda_arch.clone(),
            "__linux" => linux.clone(),
            "__osx" => osx.clone(),
            "__win" => win.clone(),
            // The libc family is handled by `apply_glibc_override` below, since
            // the single glibc env var must not rewrite `__musl`/`__eglibc`.
            _ => VersionOverride::Untouched,
        };
        match outcome {
            VersionOverride::Pinned(version) => {
                package.version = version;
                true
            }
            VersionOverride::Disabled => false,
            VersionOverride::Untouched => true,
        }
    });

    // Overrides can introduce packages the machine lacks (`CONDA_OVERRIDE_CUDA`
    // without a GPU), matching rattler; the `Ok(None)` fallback adds only set vars.
    // `carried_by_subdir` keeps an override from inventing an OS package the
    // target can't have, the way rattler gates the same slots on the platform.
    let mut add_missing = |name: &str, version: Option<Version>| {
        let Some(version) = version else { return };
        if !carried_by_subdir(name, subdir) {
            return;
        }
        if packages.iter().any(|p| p.name.as_normalized() == name) {
            return;
        }
        packages.push(GenericVirtualPackage {
            name: name.parse().expect("static virtual package name is valid"),
            version,
            build_string: "0".to_string(),
        });
    };

    add_missing("__cuda", cuda.pinned());
    add_missing("__cuda_arch", cuda_arch.pinned());
    add_missing("__osx", osx.pinned());
    add_missing("__linux", linux.pinned());
    add_missing("__win", win.pinned());

    // CEP couples the two CUDA slots: `__cuda_arch` is meaningless without a
    // driver, and rattler drops it the same way in `VirtualPackages::detect`.
    if !packages.iter().any(|p| p.name.as_normalized() == "__cuda") {
        packages.retain(|p| p.name.as_normalized() != "__cuda_arch");
    }

    apply_glibc_override(packages, subdir);
    apply_archspec_override(packages);
}

/// Apply `CONDA_OVERRIDE_ARCHSPEC` to `packages`: unset leaves the detected
/// `__archspec` untouched, an empty value removes it, and a microarchitecture
/// name (or `0` for "unknown") replaces or inserts it.
///
/// Unknown names are rejected by [`Archspec::parse_version`] and ignored with a
/// warning.
fn apply_archspec_override(packages: &mut Vec<GenericVirtualPackage>) {
    if std::env::var_os(Archspec::DEFAULT_ENV_NAME).is_none() {
        return;
    }
    let overridden = match Archspec::detect_with_fallback(&Override::DefaultEnvVar, || Ok(None)) {
        Ok(overridden) => overridden,
        Err(error) => {
            tracing::warn!("Ignoring {}: {error}", Archspec::DEFAULT_ENV_NAME);
            return;
        }
    };

    // Set empty: no `__archspec` at all.
    let Some(overridden) = overridden else {
        packages.retain(|p| p.name.as_normalized() != "__archspec");
        return;
    };
    // CEP 30 reserves version 0 for a build string that echoes the subdir
    // architecture. So replace version as well as build string.
    let overridden = GenericVirtualPackage::from(overridden);
    match packages
        .iter_mut()
        .find(|p| p.name.as_normalized() == "__archspec")
    {
        Some(existing) => *existing = overridden,
        None => packages.push(overridden),
    }
}

/// Apply `CONDA_OVERRIDE_GLIBC` (rattler's only libc slot) to `packages`. The
/// glibc env var governs glibc alone: unset leaves libc packages untouched, an
/// empty value removes `__glibc`, and a concrete version pins
/// `__glibc=<version>=0` and drops `__musl`/`__eglibc` (one libc family
/// applies).
fn apply_glibc_override(packages: &mut Vec<GenericVirtualPackage>, subdir: Platform) {
    // Read the variable rattler would and reuse its empty-vs-version parsing.
    let Ok(value) = std::env::var(LibC::DEFAULT_ENV_NAME) else {
        return;
    };
    if !carried_by_subdir("__glibc", subdir) {
        return;
    }
    match LibC::parse_version_opt(&value) {
        // `CONDA_OVERRIDE_GLIBC=""`: drop `__glibc`, leave `__musl`/`__eglibc`.
        Ok(None) => packages.retain(|p| p.name.as_normalized() != "__glibc"),
        // `CONDA_OVERRIDE_GLIBC=<version>`: glibc becomes the active libc.
        Ok(Some(libc)) => {
            packages.retain(|p| !matches!(p.name.as_normalized(), "__musl" | "__eglibc"));
            if let Some(glibc) = packages
                .iter_mut()
                .find(|p| p.name.as_normalized() == "__glibc")
            {
                glibc.version = libc.version;
                glibc.build_string = "0".to_string();
            } else {
                packages.push(GenericVirtualPackage {
                    name: "__glibc"
                        .parse()
                        .expect("static virtual package name is valid"),
                    version: libc.version,
                    build_string: "0".to_string(),
                });
            }
        }
        // Unusable value: leave the detected packages untouched, but say so.
        Err(error) => {
            tracing::warn!("Ignoring {}: {error}", LibC::DEFAULT_ENV_NAME);
        }
    }
}

/// What pixi assumes about the subdir we target, as a platform.
///
/// This is the stand-in for callers that need *a* platform when no declared one
/// matches; callers that need the machine itself want [`detect_host`].
pub fn host_baseline() -> PixiPlatform {
    subdir_baseline(host_subdir())
}

/// `subdir`'s defaults with `CONDA_OVERRIDE_*` on top.
///
/// Without an override that is the plain subdir platform. An override makes it
/// a rich platform, because a subdir-named entry has to carry exactly the
/// subdir defaults.
fn subdir_baseline(subdir: Platform) -> PixiPlatform {
    let mut virtual_packages = PixiPlatform::from_subdir(subdir)
        .declared_virtual_packages()
        .to_vec();
    apply_conda_overrides(&mut virtual_packages, subdir);
    platform_from_detected(subdir, virtual_packages)
        .unwrap_or_else(|_| PixiPlatform::from_subdir(subdir))
}

/// This machine as a platform targeting `subdir`.
///
/// For a subdir this machine runs, that is what rattler detects with
/// `CONDA_OVERRIDE_*` on top. Detection runs *for the subdir* rather than for
/// `Platform::current()`, so a `PIXI_OVERRIDE_PLATFORM` target is not labelled
/// with the real machine's architecture.
///
/// For any other subdir there is nothing to detect - a Linux box cannot report
/// a macOS version - so the answer is that subdir's baseline, the assumption
/// [`host_baseline`] makes for the subdir we target. The near-empty set
/// detection returns instead would leave every declared platform unsatisfied
/// and make `PIXI_OVERRIDE_PLATFORM` useless for anything but a bare subdir.
pub fn detect_host(subdir: Platform) -> Result<PixiPlatform, HostDetectionError> {
    if !machine_runs(subdir) {
        return Ok(subdir_baseline(subdir));
    }
    Ok(platform_from_detected(subdir, probe_machine(subdir)?)?)
}

/// Whether this machine can run packages from `subdir`.
///
/// It is the same [`candidate_subdirs`] test that decides which declared
/// platforms this host may select, so a reading taken here describes a machine
/// that really executes those packages: `osx-64` on Apple Silicon reports the
/// true macOS version, while `linux-aarch64` on an x86 box reports nothing,
/// rather than lending it this machine's glibc and kernel.
fn machine_runs(subdir: Platform) -> bool {
    candidate_subdirs(Platform::current()).contains(&subdir)
}

/// The raw virtual packages rattler reports for `subdir`, with
/// `CONDA_OVERRIDE_*` applied. Sparse for a subdir this machine cannot run,
/// which is why [`detect_host`] falls back to the baseline there and
/// [`machine_virtual_packages`] answers with nothing at all.
fn probe_machine(subdir: Platform) -> Result<Vec<GenericVirtualPackage>, HostDetectionError> {
    let mut detected =
        VirtualPackages::detect_for_platform(subdir, &detection_overrides(subdir), None)?
            .into_generic_virtual_packages()
            .collect::<Vec<_>>();
    // Canonicalize last: the override pass inserts rattler-shaped entries of
    // its own (`CONDA_OVERRIDE_ARCHSPEC` goes through `GenericVirtualPackage::
    // from(Archspec)`, which stamps version 1), and those need rewriting too.
    apply_conda_overrides(&mut detected, subdir);
    Ok(detected.into_iter().map(canonicalize_detected).collect())
}

/// The rattler overrides detection itself runs with.
///
/// `CONDA_OVERRIDE_*` is deliberately *not* handed to rattler here: it fails the
/// whole detection on a value it cannot parse, where [`apply_conda_overrides`]
/// validates per slot and warns. The one exception is archspec on a cross-subdir
/// target, where rattler reads `CONDA_OVERRIDE_ARCHSPEC` on its own and aborts
/// on a bad value, so pin the slot to the subdir's own architecture and let the
/// per-slot pass override it.
fn detection_overrides(subdir: Platform) -> VirtualPackageOverrides {
    let mut overrides = VirtualPackageOverrides::default();
    if subdir != Platform::current() {
        overrides.archspec = Some(Override::String(
            Archspec::from_platform(subdir).map_or_else(
                || String::from("0"),
                |archspec| archspec.as_str().to_string(),
            ),
        ));
    }
    overrides
}

/// Assemble `detected` into a workspace-registrable platform for `subdir`.
///
/// The pure half of [`detect_host`], split out so the assembly rules are
/// testable without probing the machine or the environment.
///
/// `detected` is the machine's *complete* answer, so it is declared verbatim
/// rather than merged over the subdir defaults. Merging would put back a
/// package the machine does not have: `CONDA_OVERRIDE_GLIBC=""` says this
/// machine has no glibc, and re-seeding `__glibc = "2.28"` from the defaults
/// would contradict it. A manifest entry is the opposite case - a user writes
/// only the keys they care about - which is why
/// [`PixiPlatform::new_with_defaults`] merges and this does not.
///
/// A machine reporting exactly the subdir defaults is the subdir platform.
pub fn platform_from_detected(
    subdir: Platform,
    detected: Vec<GenericVirtualPackage>,
) -> Result<PixiPlatform, PixiPlatformError> {
    let declared: Vec<GenericVirtualPackage> =
        detected.into_iter().map(canonicalize_detected).collect();

    if same_set(&declared, &subdir_default_virtual_packages(subdir)) {
        return Ok(PixiPlatform::from_subdir(subdir));
    }

    let customised: Vec<GenericVirtualPackage> = declared
        .iter()
        .filter(|gvp| !is_subdir_default(gvp, subdir))
        .cloned()
        .collect();
    let name = crate::platform::synthesized_name(subdir, &customised)?;

    // The name is synthesized from the customised packages alone, so a machine
    // that only *drops* a default (an empty `CONDA_OVERRIDE_*`) and matches the
    // baseline otherwise has no name of its own to take. Nothing in the model
    // spells "this subdir, minus one of its defaults", so fall back to the
    // subdir platform there rather than invent one.
    PixiPlatform::new(name, subdir, declared).or_else(|error| match error {
        PixiPlatformError::IsSubdirPlatform => Ok(PixiPlatform::from_subdir(subdir)),
        other => Err(other),
    })
}

/// Whether two virtual-package lists hold the same entries, order aside.
fn same_set(left: &[GenericVirtualPackage], right: &[GenericVirtualPackage]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut left: Vec<&GenericVirtualPackage> = left.iter().collect();
    let mut right: Vec<&GenericVirtualPackage> = right.iter().collect();
    left.sort();
    right.sort();
    left == right
}

/// Rewrite a rattler-detected virtual package into the shape the manifest uses
/// for the same package.
///
/// The two disagree on placeholders. Rattler stamps `"0"` as the build string
/// of every version-carrying package and encodes `__archspec` as version 1 with
/// the microarchitecture in the build string, while the manifest writes an
/// empty build string and pins `__archspec` to version 0 (`__unix` is built as
/// `0=0` on both sides). Left unreconciled, a detected package never compares
/// equal to the subdir default it *is*: a machine matching the baseline would
/// still produce a rich platform, and `__archspec` would serialize through the
/// raw `__archspec = "1=zen2"` escape hatch instead of the friendly
/// `archspec = "zen2"` form.
///
/// Nothing is lost on the way to the solver: `get_minimal_virtual_packages`
/// rebuilds the typed [`rattler_virtual_packages::VirtualPackage`] from the
/// name and build string, and rattler stamps its own version back on.
fn canonicalize_detected(gvp: GenericVirtualPackage) -> GenericVirtualPackage {
    match gvp.name.as_normalized() {
        "__unix" => gvp,
        "__archspec" => GenericVirtualPackage {
            version: Version::major(0),
            ..gvp
        },
        _ if gvp.build_string == "0" => GenericVirtualPackage {
            build_string: String::new(),
            ..gvp
        },
        _ => gvp,
    }
}

/// The virtual packages this machine provides, for callers that ask "does the
/// host satisfy this declared platform?".
///
/// A detection failure warns once and yields an empty list, which fails closed:
/// no declared platform's requirements are met, rather than pixi assuming its
/// defaults for a machine it could not read. Callers whose result depends on
/// getting this right - anything that solves or installs - must use
/// [`detect_host`] and propagate the error instead.
pub fn host_capabilities() -> Vec<GenericVirtualPackage> {
    // Warned at most once per process, so a run that inspects several
    // environments does not repeat a machine-wide failure.
    static DETECTION_WARNING: std::sync::Once = std::sync::Once::new();

    match detect_host(host_subdir()) {
        Ok(platform) => platform.declared_virtual_packages().to_vec(),
        Err(error) => {
            DETECTION_WARNING.call_once(|| {
                tracing::warn!("Could not detect the virtual packages of this machine: {error}");
            });
            Vec::new()
        }
    }
}

/// What this machine reports about `subdir`, for display only.
///
/// Unlike [`host_capabilities`] this never substitutes pixi's assumptions:
/// asked about a subdir this machine cannot run it answers with nothing at all,
/// because a field that says "detected" must not show numbers nothing
/// detected.
pub fn machine_virtual_packages(subdir: Platform) -> Vec<GenericVirtualPackage> {
    // Warned separately from the one in `host_capabilities`, so a failure to
    // *display* the machine does not silence the one that changes which
    // platform is selected.
    static DISPLAY_WARNING: std::sync::Once = std::sync::Once::new();

    // Detection for a foreign subdir does not fail, it *invents*: rattler
    // answers `__win` on a Linux box, and `detection_overrides` supplies the
    // architecture itself. Neither is something this machine reported.
    if !machine_runs(subdir) {
        return Vec::new();
    }
    probe_machine(subdir).unwrap_or_else(|error| {
        DISPLAY_WARNING.call_once(|| {
            tracing::warn!("Could not detect the virtual packages of this machine: {error}");
        });
        Vec::new()
    })
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_conda_types::PackageName;

    use super::*;
    use crate::{PixiPlatformName, PixiPlatformNameError};

    /// A virtual package in the shape rattler hands back from detection: a
    /// `"0"` build string on everything that carries a version.
    fn detected(name: &str, version: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: PackageName::try_from(name).unwrap(),
            version: Version::from_str(version).unwrap(),
            build_string: "0".to_string(),
        }
    }

    fn detected_archspec(microarchitecture: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: PackageName::try_from("__archspec").unwrap(),
            version: Version::major(1),
            build_string: microarchitecture.to_string(),
        }
    }

    fn declared(platform: &PixiPlatform, name: &str) -> Option<String> {
        platform
            .declared_virtual_packages()
            .iter()
            .find(|gvp| gvp.name.as_normalized() == name)
            .map(|gvp| gvp.version.to_string())
    }

    /// `CONDA_OVERRIDE_*` must be able to *introduce* a virtual package the
    /// machine doesn't provide (e.g. cuda on a GPU-less box), not just
    /// override detected ones.
    #[test]
    fn override_adds_undetected_virtual_package() {
        let packages = temp_env::with_var("CONDA_OVERRIDE_CUDA", Some("12.0"), || {
            let mut packages = Vec::new();
            apply_conda_overrides(&mut packages, Platform::Linux64);
            packages
        });

        let cuda = packages
            .iter()
            .find(|p| p.name.as_normalized() == "__cuda")
            .expect("__cuda should be added from the override");
        assert_eq!(cuda.version, Version::from_str("12.0").unwrap());
    }

    fn libc_package(name: &str, version: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: name.parse().unwrap(),
            version: Version::from_str(version).unwrap(),
            build_string: "0".to_string(),
        }
    }

    fn has_package(packages: &[GenericVirtualPackage], name: &str) -> bool {
        packages.iter().any(|p| p.name.as_normalized() == name)
    }

    /// An empty `CONDA_OVERRIDE_GLIBC` drops `__glibc` but must leave a
    /// non-glibc libc family (here `__musl`) untouched -- the glibc slot only
    /// governs glibc.
    #[test]
    fn empty_glibc_override_drops_glibc_but_keeps_musl() {
        let packages = temp_env::with_var("CONDA_OVERRIDE_GLIBC", Some(""), || {
            let mut packages = vec![
                libc_package("__glibc", "2.28"),
                libc_package("__musl", "1.2"),
            ];
            apply_conda_overrides(&mut packages, Platform::Linux64);
            packages
        });

        assert!(!has_package(&packages, "__glibc"));
        assert!(has_package(&packages, "__musl"));
    }

    /// A `CONDA_OVERRIDE_GLIBC` version makes glibc the active libc: it pins
    /// `__glibc=<version>=0` and displaces any detected `__musl`/`__eglibc`.
    #[test]
    fn glibc_version_override_displaces_other_libc_families() {
        let packages = temp_env::with_var("CONDA_OVERRIDE_GLIBC", Some("2.40"), || {
            let mut packages = vec![
                libc_package("__musl", "1.2"),
                libc_package("__eglibc", "2.30"),
            ];
            apply_conda_overrides(&mut packages, Platform::Linux64);
            packages
        });

        assert!(!has_package(&packages, "__musl"));
        assert!(!has_package(&packages, "__eglibc"));
        let glibc = packages
            .iter()
            .find(|p| p.name.as_normalized() == "__glibc")
            .expect("a glibc version override should add __glibc");
        assert_eq!(glibc.version, Version::from_str("2.40").unwrap());
        assert_eq!(glibc.build_string, "0");
    }

    /// A machine more capable than the subdir baseline keeps its own values,
    /// and the packages it never spoke to are filled in from the defaults.
    #[test]
    fn host_platform_keeps_detected_values_over_defaults() {
        let platform = platform_from_detected(
            Platform::Linux64,
            vec![
                detected("__unix", "0"),
                detected("__linux", "7.1.8"),
                detected("__glibc", "2.42"),
                detected_archspec("zen2"),
            ],
        )
        .unwrap();

        assert_eq!(declared(&platform, "__linux").as_deref(), Some("7.1.8"));
        assert_eq!(declared(&platform, "__glibc").as_deref(), Some("2.42"));
        assert_eq!(declared(&platform, "__unix").as_deref(), Some("0"));
        assert!(!platform.is_subdir_platform());
    }

    /// Detection can report *less* than pixi assumes. The machine wins there
    /// too, so the environment is solved for the machine it will run on
    /// rather than for a baseline it does not meet.
    #[test]
    fn host_platform_narrows_below_the_defaults() {
        let platform = platform_from_detected(
            Platform::Linux64,
            vec![detected("__unix", "0"), detected("__glibc", "2.17")],
        )
        .unwrap();

        assert_eq!(declared(&platform, "__glibc").as_deref(), Some("2.17"));
    }

    /// A musl host reports `__musl` and must not also acquire the `__glibc`
    /// default: one libc family applies.
    #[test]
    fn host_platform_keeps_musl_without_glibc() {
        let platform = platform_from_detected(
            Platform::Linux64,
            vec![detected("__unix", "0"), detected("__musl", "1.2.4")],
        )
        .unwrap();

        assert_eq!(declared(&platform, "__musl").as_deref(), Some("1.2.4"));
        assert_eq!(declared(&platform, "__glibc"), None);
    }

    /// An empty `CONDA_OVERRIDE_*` says the machine does not have that package
    /// at all. The subdir default must not be put back in its place.
    #[test]
    fn host_platform_keeps_a_disabled_package_out() {
        let platform = platform_from_detected(
            Platform::Linux64,
            vec![
                detected("__unix", "0"),
                detected("__linux", "7.1.8"),
                detected_archspec("zen2"),
            ],
        )
        .unwrap();

        assert_eq!(declared(&platform, "__glibc"), None);
        assert_eq!(declared(&platform, "__linux").as_deref(), Some("7.1.8"));
    }

    /// A loaded machine spells out a long name - CUDA and a compute
    /// capability on top of the usual four packages - and the name still has
    /// to be one the manifest can read back.
    #[test]
    fn host_platform_name_stays_within_the_limit() {
        let platform = platform_from_detected(
            Platform::Linux64,
            vec![
                detected("__unix", "0"),
                detected("__linux", "7.1.8"),
                detected("__glibc", "2.42"),
                detected_archspec("zen2"),
                detected("__cuda", "12.0"),
                detected("__cuda_arch", "8.6"),
            ],
        )
        .unwrap();

        let name = platform.name().as_str();
        assert!(
            PixiPlatformName::try_from(name).is_ok(),
            "synthesized name is not a valid platform name: {name}"
        );
        // The name is a pure function of the definition, so it survives the
        // round trip that `has_derived_name` and the lock alignment rely on.
        assert!(platform.has_derived_name(), "got {name}");
    }

    /// Detected packages are not always pixi's own: a lock file's platform row
    /// carries whatever was written into it, and a long enough package name
    /// spells out past the limit. That platform has no name, which is an error
    /// the caller can drop the row over - never a panic.
    #[test]
    fn a_platform_that_cannot_be_named_is_an_error() {
        let unnameable = format!("__{}", "a".repeat(120));
        let error = platform_from_detected(
            Platform::Linux64,
            vec![
                detected("__unix", "0"),
                detected("__linux", "7.1.8"),
                detected("__glibc", "2.42"),
                detected_archspec("zen2"),
                detected(&unnameable, "1"),
            ],
        )
        .expect_err("a 120-byte virtual package cannot fit in a platform name");

        assert!(
            matches!(
                error,
                PixiPlatformError::Name(PixiPlatformNameError::TooLong { .. })
            ),
            "got {error:?}"
        );
    }

    /// A machine that reports exactly the subdir baseline is the subdir
    /// platform -- detection should not manufacture a rich platform whose
    /// customisations are all defaults.
    #[test]
    fn host_platform_matching_the_defaults_collapses_to_the_subdir() {
        let defaults = subdir_default_virtual_packages(Platform::Linux64)
            .into_iter()
            .map(|gvp| {
                // Restate them the way rattler would hand them over.
                if gvp.name.as_normalized() == "__archspec" {
                    detected_archspec(&gvp.build_string)
                } else if gvp.build_string.is_empty() {
                    GenericVirtualPackage {
                        build_string: "0".to_string(),
                        ..gvp
                    }
                } else {
                    gvp
                }
            })
            .collect();

        let platform = platform_from_detected(Platform::Linux64, defaults).unwrap();
        assert!(platform.is_subdir_platform(), "got {platform:?}");
    }
}
