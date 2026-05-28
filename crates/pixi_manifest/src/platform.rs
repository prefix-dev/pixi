use std::fmt::{self, Display, Formatter};
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::str::FromStr;

use pixi_default_versions::{
    default_glibc_version, default_linux_version, default_mac_os_version, default_windows_version,
};
use rattler_conda_types::{GenericVirtualPackage, PackageName, Platform, Version};
use rattler_virtual_packages::{
    Archspec, DetectVirtualPackageError, Override, VirtualPackageOverrides, VirtualPackages,
};

use crate::TargetSelector;

#[derive(thiserror::Error, Clone, Debug)]
pub enum PixiPlatformNameError {
    #[error("a platform name can not be empty")]
    Empty,
    #[error("a platform name can not contain '{character}' at position {position}")]
    InvalidCharacter { character: char, position: usize },
    #[error("'{0}' is a reserved platform name")]
    ReservedName(String),
    #[error("a platform name can not be longer than {max} bytes (got {actual})")]
    TooLong { max: usize, actual: usize },
}

/// Cap names so attacker-controlled manifests can't pass unbounded keys.
/// Longest real conda subdir is 17 bytes; 64 is comfortable for descriptive
/// custom names like `gpu-linux-cuda12-glibc228`.
const MAX_PLATFORM_NAME_BYTES: usize = 64;

#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct PixiPlatformName(String);

impl Display for PixiPlatformName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl PixiPlatformName {
    pub(crate) fn valid_pixi_platform_name(input: &str) -> Result<String, PixiPlatformNameError> {
        let bytes = input.as_bytes();
        let input_len = bytes.len();
        if bytes.is_empty() {
            return Err(PixiPlatformNameError::Empty);
        }
        if input_len > MAX_PLATFORM_NAME_BYTES {
            return Err(PixiPlatformNameError::TooLong {
                max: MAX_PLATFORM_NAME_BYTES,
                actual: input_len,
            });
        }
        for (pos, c) in bytes.iter().enumerate() {
            let character = if !c.is_ascii_control() && *c < 128 {
                *c as char
            } else {
                '�'
            };
            let is_first = pos == 0;
            let is_last = pos + 1 == input_len;

            let ok = if is_first {
                // Alphabetic only -- no leading digit or dash.
                c.is_ascii_alphabetic()
            } else if is_last {
                // Trailing `-` is not allowed.
                c.is_ascii_alphanumeric()
            } else {
                c.is_ascii_alphanumeric() || *c == b'-'
            };

            if !ok {
                return Err(PixiPlatformNameError::InvalidCharacter {
                    character,
                    position: pos,
                });
            }
        }
        Ok(input.to_string())
    }
}

impl TryFrom<&str> for PixiPlatformName {
    type Error = PixiPlatformNameError;

    fn try_from(input: &str) -> Result<Self, Self::Error> {
        let validated = Self::valid_pixi_platform_name(input)?;
        // Family selectors (`linux`/`unix`/`win`/`osx`/`macos`) double as
        // `target.<family>.*` keys; a platform named after one would shadow them.
        if crate::target::family_name_to_selector(&validated).is_some() {
            return Err(PixiPlatformNameError::ReservedName(validated));
        }
        Ok(PixiPlatformName(validated))
    }
}

impl From<Platform> for PixiPlatformName {
    fn from(subdir: Platform) -> Self {
        PixiPlatformName(subdir.to_string())
    }
}

impl std::str::FromStr for PixiPlatformName {
    type Err = PixiPlatformNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PixiPlatformName::try_from(s)
    }
}

impl Deref for PixiPlatformName {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(thiserror::Error, Clone, Debug)]
pub enum PixiPlatformError {
    #[error("You tried to add a virtual package into a special subdir platform")]
    IsSubdirPlatform,
}

/// A platform declared by the workspace.
///
/// A workspace platform is a named view onto a conda subdir, optionally
/// extended with a set of virtual-package specs that the platform guarantees.
/// The combination of subdir + virtual packages lets a workspace target
/// e.g. `linux-64` "twice" -- once with `__cuda` available and once without --
/// and have each variant produce its own solved environment in the lockfile.
///
/// The `name` is a workspace-scoped label (alphanumeric, `_`, `-`). Features
/// reference workspace platforms by this name. The name is also what gets
/// written into `pixi.lock` as the platform identifier.
#[derive(Debug, Clone)]
pub struct PixiPlatform {
    /// The workspace-unique name of this platform.
    name: PixiPlatformName,
    /// The conda subdir for this platform (e.g. `linux-64`).
    subdir: Platform,
    /// Virtual packages declared for this platform in `pixi.toml`. Stored
    /// verbatim as parsed from the manifest so that round-tripping through
    /// the TOML layer needs no knowledge of the concrete set of virtual
    /// packages rattler supports. The runtime conversion to
    /// [`VirtualPackageOverrides`] lives in `overrides_from_declared`.
    declared_virtual_packages: Vec<GenericVirtualPackage>,
}

impl PixiPlatform {
    /// Build a subdir `PixiPlatform`.
    ///
    /// The name equals the subdir string, and the declared virtual-package
    /// list is set to the subdir's defaults (`__unix`/`__linux`/`__glibc`/
    /// `__win`/`__osx`/`__archspec` -- whichever apply). The defaults are
    /// load-bearing: a subdir platform represents what pixi assumes about
    /// `subdir` with no customisation, and the rest of the platform API
    /// makes it impossible to mutate them away. To transition off the
    /// subdir baseline a caller must build a rich platform via
    /// [`Self::new_with_defaults`] or run an `apply_edit` that adds a
    /// non-default virtual package.
    pub fn from_subdir(subdir: Platform) -> Self {
        Self {
            name: subdir.into(),
            subdir,
            declared_virtual_packages: subdir_default_virtual_packages(subdir),
        }
    }

    /// Build a bare placeholder `PixiPlatform`.
    ///
    /// Same shape as [`Self::from_subdir`], except the declared
    /// virtual-package list stays empty -- the platform is treated as
    /// "auto-detect at use time" by [`Self::virtual_packages`] (rattler
    /// gets a clean detection with no pixi overrides) and by
    /// `pixi_core::workspace::virtual_packages::get_minimal_virtual_packages`
    /// (which fills in pixi's defaults from the subdir).
    ///
    /// Reserved for two callers:
    ///   * the `pixi workspace platform show` host-detection display, where
    ///     forcing pixi's defaults onto the actual host detection would
    ///     mask what the user's machine reports;
    ///   * the workspace fallback placeholder used while a manifest is
    ///     being read.
    ///
    /// All other paths build real subdir platforms via [`Self::from_subdir`].
    pub fn auto_detected(subdir: Platform) -> Self {
        Self {
            name: subdir.into(),
            subdir,
            declared_virtual_packages: Vec::new(),
        }
    }

    pub fn name(&self) -> &PixiPlatformName {
        &self.name
    }

    pub fn set_name(&mut self, name: PixiPlatformName) -> Result<(), PixiPlatformError> {
        // A platform without virtual packages must always be named after its
        // subdir, so a bare subdir-platform can't be renamed, and a VP-bearing
        // one can't be renamed onto its own subdir name.
        if self.is_subdir_platform()
            || (name.as_str() == self.subdir.as_str() && !self.declared_virtual_packages.is_empty())
        {
            Err(PixiPlatformError::IsSubdirPlatform)
        } else {
            self.name = name;
            Ok(())
        }
    }

    pub fn subdir(&self) -> Platform {
        self.subdir
    }

    pub fn set_subdir(&mut self, subdir: Platform) -> Result<(), PixiPlatformError> {
        if self.is_subdir_platform() {
            Err(PixiPlatformError::IsSubdirPlatform)
        } else {
            self.subdir = subdir;
            Ok(())
        }
    }

    pub fn is_subdir_platform(&self) -> bool {
        self.subdir.as_str() == self.name.as_str()
    }

    /// Build a new `PixiPlatform`.
    ///
    /// Enforces the workspace invariant that a subdir-platform (entry where
    /// `name == subdir`) carries exactly the subdir defaults, no more and
    /// no less. A subdir-named entry with a custom virtual-package list has
    /// no syntactic shape in `pixi.toml` (bare-string form has no slot for
    /// customisations) and the target-selector machinery treats it as the
    /// bare subdir alias; allowing one in memory would make the model
    /// inconsistent with what can be written to disk and read back.
    ///
    /// To accept user input that the caller hasn't pre-loaded with the
    /// subdir defaults, use [`Self::new_with_defaults`] instead -- it merges
    /// the defaults under the user's overrides and routes the
    /// `name == subdir` case through [`Self::from_subdir`].
    pub fn new(
        name: PixiPlatformName,
        subdir: Platform,
        declared_virtual_packages: Vec<GenericVirtualPackage>,
    ) -> Result<Self, PixiPlatformError> {
        if name.as_str() == subdir.as_str()
            && declared_virtual_packages != subdir_default_virtual_packages(subdir)
        {
            return Err(PixiPlatformError::IsSubdirPlatform);
        }
        Ok(Self {
            name,
            subdir,
            declared_virtual_packages,
        })
    }

    /// Build a new `PixiPlatform`, materialising the subdir's default virtual
    /// packages alongside `user_declared`. Entries the user supplied win over
    /// the default by virtual-package name, so a `__glibc = "2.31"` in
    /// `user_declared` keeps that version even though the subdir defaults
    /// would otherwise inject `__glibc = "2.28"`.
    ///
    /// When `name == subdir` and `user_declared` is empty, this returns the
    /// subdir platform produced by [`Self::from_subdir`] (which carries the
    /// subdir defaults). Customising a subdir-named entry is rejected with
    /// [`PixiPlatformError::IsSubdirPlatform`] -- the subdir baseline is
    /// fixed, callers that want to customise must give the platform a
    /// name distinct from its subdir.
    pub fn new_with_defaults(
        name: PixiPlatformName,
        subdir: Platform,
        user_declared: Vec<GenericVirtualPackage>,
    ) -> Result<Self, PixiPlatformError> {
        if name.as_str() == subdir.as_str() {
            if !user_declared.is_empty() {
                return Err(PixiPlatformError::IsSubdirPlatform);
            }
            return Ok(Self::from_subdir(subdir));
        }
        let mut declared = user_declared;
        merge_subdir_defaults(&mut declared, subdir);
        Self::new(name, subdir, declared)
    }

    pub fn as_target_selector(&self) -> TargetSelector {
        if self.subdir.as_str() == *self.name {
            TargetSelector::Subdir(self.subdir)
        } else {
            TargetSelector::Platform(self.name.clone())
        }
    }

    pub fn virtual_packages(&self) -> Result<VirtualPackages, DetectVirtualPackageError> {
        let overrides = overrides_from_declared(&self.declared_virtual_packages);
        VirtualPackages::detect_for_platform(self.subdir, &overrides)
    }

    pub fn set_declared_virtual_packages(
        &mut self,
        declared_virtual_packages: Vec<GenericVirtualPackage>,
    ) -> Result<(), PixiPlatformError> {
        if self.is_subdir_platform() {
            Err(PixiPlatformError::IsSubdirPlatform)
        } else {
            self.declared_virtual_packages = declared_virtual_packages;
            Ok(())
        }
    }

    pub fn declared_virtual_packages(&self) -> &[GenericVirtualPackage] {
        &self.declared_virtual_packages
    }

    /// Apply an in-place edit to this platform.
    ///
    /// Operations are applied in this order so the result is independent of
    /// argument ordering: clear the VP list (if requested), then for each entry
    /// in `insert_or_update_virtual_packages` replace an existing VP with the
    /// same name or push it if absent, then remove any VPs whose name is in
    /// `remove_virtual_packages`, then set the subdir if provided.
    ///
    /// Subdir platforms (entries where `name == subdir`) carry an immutable
    /// set of defaults. An edit on a subdir platform is rejected unless its
    /// upsert list contains at least one virtual package that differs from
    /// the subdir defaults -- those edits transition the platform to a
    /// rich entry, which is the only legal "change" to a subdir platform.
    /// Anything else (removing or re-stating a default, changing the
    /// subdir, clearing the VP list) is rejected with
    /// [`PixiPlatformError::IsSubdirPlatform`].
    ///
    /// On rich platforms:
    /// - the synthesised auto-name is recomputed from the new subdir/VPs;
    /// - an edit that strips every virtual package collapses the platform
    ///   back to a subdir platform (the defaults are re-materialised from
    ///   the subdir);
    /// - the subdir defaults are always re-merged after the edit so a
    ///   `--remove-virtual-package` pass that strips a default value
    ///   leaves it re-seeded from the subdir rather than absent.
    pub fn apply_edit(&mut self, edit: PlatformEdit) -> Result<(), PixiPlatformError> {
        let was_subdir = self.is_subdir_platform();
        if was_subdir {
            // Subdir platforms can only be transformed into rich entries.
            // The single signal that the caller wants that transformation
            // is an upsert whose virtual package differs from the subdir
            // defaults. Without it the edit either restates a default
            // (no effect after the defaults re-merge), or tries to mutate
            // the immutable subdir baseline -- both are rejected.
            let target_subdir = edit.set_subdir.unwrap_or(self.subdir);
            let has_customisation = edit
                .insert_or_update_virtual_packages
                .iter()
                .any(|gvp| !is_subdir_default(gvp, target_subdir));
            if !has_customisation {
                return Err(PixiPlatformError::IsSubdirPlatform);
            }
        }
        // A subdir platform and a synthesised platform both carry the
        // auto-derived name; only an explicit custom name differs from it, and
        // such a name is preserved across the edit.
        let was_auto = self.name.as_str()
            == crate::toml::platform::synthesize_name_string(
                self.subdir,
                &self.declared_virtual_packages,
            );
        // The subdir might be about to change. Capture it so we can strip
        // the old subdir's defaults from `declared` before merging the new
        // subdir's defaults -- otherwise a Linux64 → Osx64 set_subdir
        // would leave `__linux` and `__glibc` materialised on an osx
        // entry where they don't belong.
        let old_subdir = self.subdir;

        if edit.clear_virtual_packages {
            self.declared_virtual_packages.clear();
        }

        for upsert in edit.insert_or_update_virtual_packages {
            if let Some(existing) = self
                .declared_virtual_packages
                .iter_mut()
                .find(|gvp| gvp.name == upsert.name)
            {
                *existing = upsert;
            } else {
                self.declared_virtual_packages.push(upsert);
            }
        }

        if !edit.remove_virtual_packages.is_empty() {
            self.declared_virtual_packages
                .retain(|gvp| !edit.remove_virtual_packages.contains(&gvp.name));
        }

        if let Some(subdir) = edit.set_subdir {
            self.subdir = subdir;
        }

        if self.subdir != old_subdir {
            // Strip the previous subdir's defaults so they don't leak onto
            // the new subdir. Custom values the user explicitly set (a
            // non-default version) survive -- only entries that match the
            // old subdir's default exactly are stripped.
            self.declared_virtual_packages
                .retain(|gvp| !is_subdir_default(gvp, old_subdir));
        }

        if self.declared_virtual_packages.is_empty() {
            // Dropping the last virtual package collapses a rich platform
            // back to a subdir platform: the name resets to the subdir,
            // and the subdir defaults are re-materialised so the
            // subdir-platform invariant holds.
            *self = Self::from_subdir(self.subdir);
            return Ok(());
        }

        // The platform is rich after the edit; make sure the subdir defaults
        // are materialised alongside whatever the edit produced. Any user
        // entry with the same name wins, so an `--remove-virtual-package`
        // pass that strips a default value will be re-seeded from the subdir
        // default rather than left absent.
        merge_subdir_defaults(&mut self.declared_virtual_packages, self.subdir);

        if was_auto {
            // Recompute the synthesised name from the new subdir + VPs. The
            // synthesiser only emits valid names (subdir prefix, sanitised
            // segments), so it needs no validation.
            self.name = PixiPlatformName(crate::toml::platform::synthesize_name_string(
                self.subdir,
                &self.declared_virtual_packages,
            ));
        } else if self.name.as_str() == self.subdir.as_str() {
            // A preserved custom name that now equals the subdir while VPs
            // remain would forge an illegal subdir-platform.
            return Err(PixiPlatformError::IsSubdirPlatform);
        }

        Ok(())
    }
}

/// A set of changes to apply to an existing [`PixiPlatform`].
///
/// Used by [`PixiPlatform::apply_edit`] and the manifest-level platform editor.
/// Default value is a no-op.
#[derive(Debug, Default, Clone)]
pub struct PlatformEdit {
    /// New value for `subdir`. Unset means "leave alone".
    pub set_subdir: Option<Platform>,
    /// When `true`, drop the existing virtual-package list before applying
    /// the insert-or-update list below. Used by `--clear-virtual-packages`.
    pub clear_virtual_packages: bool,
    /// Virtual packages to add or, when a package with the same name already
    /// exists, replace.
    pub insert_or_update_virtual_packages: Vec<GenericVirtualPackage>,
    /// Virtual packages to remove by name (no-op if not present).
    pub remove_virtual_packages: Vec<PackageName>,
}

impl PlatformEdit {
    pub fn is_noop(&self) -> bool {
        self.set_subdir.is_none()
            && !self.clear_virtual_packages
            && self.insert_or_update_virtual_packages.is_empty()
            && self.remove_virtual_packages.is_empty()
    }
}

/// Return the set of virtual packages pixi treats as defaults for `subdir`.
///
/// Mirrors the subdir-driven entries that `pixi_core`'s
/// `get_minimal_virtual_packages` produces (the conda-lock-compatible minimal
/// set): `__unix` on unix subdirs, `__linux` + `__glibc` on linux, `__win` on
/// windows, `__osx` on osx, and `__archspec` wherever rattler knows the
/// minimum microarchitecture. `__cuda` is never a default -- it's opt-in.
fn subdir_default_virtual_packages(subdir: Platform) -> Vec<GenericVirtualPackage> {
    fn version_pkg(name: &str, version: Version) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: PackageName::try_from(name).expect("static virtual-package name"),
            version,
            build_string: String::new(),
        }
    }

    let mut defaults = Vec::new();

    if subdir.is_unix() {
        defaults.push(GenericVirtualPackage {
            name: PackageName::try_from("__unix").expect("static virtual-package name"),
            version: Version::major(0),
            build_string: "0".to_string(),
        });
    }
    if subdir.is_linux() {
        defaults.push(version_pkg("__linux", default_linux_version()));
        defaults.push(version_pkg("__glibc", default_glibc_version()));
    }
    if subdir.is_windows() {
        defaults.push(version_pkg("__win", default_windows_version()));
    }
    if subdir.is_osx() {
        defaults.push(version_pkg("__osx", default_mac_os_version(subdir)));
    }
    if let Some(spec) = Archspec::from_platform(subdir) {
        defaults.push(GenericVirtualPackage {
            name: PackageName::try_from("__archspec").expect("static virtual-package name"),
            version: Version::major(0),
            build_string: spec.as_str().to_string(),
        });
    }

    defaults
}

/// Returns `true` if `gvp` is exactly the value `subdir_default_virtual_packages`
/// would emit for `subdir`. Used by the TOML layer to elide default-matching
/// virtual packages from synthesised names and on-disk serialisation, and by
/// the lock-file satisfiability check to compare only the user-customised
/// virtual packages.
pub fn is_subdir_default(gvp: &GenericVirtualPackage, subdir: Platform) -> bool {
    subdir_default_virtual_packages(subdir).iter().any(|d| {
        d.name == gvp.name && d.version == gvp.version && d.build_string == gvp.build_string
    })
}

/// Parse a virtual-package entry the way it's stored in `pixi.lock` -- either
/// `__name=version` or `__name=version=build_string` -- back into a
/// [`GenericVirtualPackage`]. The lock-file serializer uses the same shape
/// rattler emits via `GenericVirtualPackage::Display`. Returns `None` if the
/// input doesn't have the expected `=`-separated form (we don't trust pixi to
/// repair a malformed lock-file entry from the satisfiability path).
pub fn parse_locked_virtual_package(raw: &str) -> Option<GenericVirtualPackage> {
    let mut parts = raw.splitn(3, '=');
    let name_str = parts.next()?;
    let version_str = parts.next()?;
    let build_string = parts.next().unwrap_or("").to_string();
    let name = PackageName::try_from(name_str).ok()?;
    let version = Version::from_str(version_str).ok()?;
    Some(GenericVirtualPackage {
        name,
        version,
        build_string,
    })
}

/// Insert any subdir default that is not already present in `declared` (by
/// virtual-package name). Entries already in `declared` win, so a user-set
/// `__linux = "5.10"` is preserved untouched. The conda-libc family is
/// special-cased: a user-supplied `__musl`/`__eglibc` replaces the default
/// `__glibc` (rattler models all three as the same `libc` override slot).
pub(crate) fn merge_subdir_defaults(declared: &mut Vec<GenericVirtualPackage>, subdir: Platform) {
    let has_libc = declared
        .iter()
        .any(|gvp| matches!(gvp.name.as_normalized(), "__glibc" | "__musl" | "__eglibc"));
    for default in subdir_default_virtual_packages(subdir) {
        if default.name.as_normalized() == "__glibc" && has_libc {
            continue;
        }
        if declared.iter().any(|gvp| gvp.name == default.name) {
            continue;
        }
        declared.push(default);
    }
}

/// Translate the manifest-declared virtual packages into the typed override
/// shape rattler expects for detection.
///
/// This is the single place in pixi that needs to know which conda virtual
/// package names map to which slot of [`VirtualPackageOverrides`]. Names that
/// have no override slot (`__unix`) or that rattler doesn't recognize are
/// dropped -- they round-trip through TOML but have no effect at detection.
fn overrides_from_declared(declared: &[GenericVirtualPackage]) -> VirtualPackageOverrides {
    let mut overrides = VirtualPackageOverrides::default();
    for gvp in declared {
        match gvp.name.as_normalized() {
            "__win" => overrides.win = Some(Override::String(gvp.version.to_string())),
            "__osx" => overrides.osx = Some(Override::String(gvp.version.to_string())),
            "__linux" => overrides.linux = Some(Override::String(gvp.version.to_string())),
            "__cuda" => overrides.cuda = Some(Override::String(gvp.version.to_string())),
            "__archspec" => {
                let value = if gvp.build_string.is_empty() || gvp.build_string == "0" {
                    "0".to_string()
                } else {
                    gvp.build_string.clone()
                };
                overrides.archspec = Some(Override::String(value));
            }
            // Upstream's `LibC::parse_version` hardcodes `family = "glibc"`,
            // so the family in the name is not preserved at detection time.
            other if other.starts_with("__") && other != "__unix" => {
                overrides.libc = Some(Override::String(gvp.version.to_string()));
            }
            _ => {}
        }
    }
    overrides
}

impl From<Platform> for PixiPlatform {
    fn from(subdir: Platform) -> Self {
        Self::from_subdir(subdir)
    }
}

impl PartialOrd for PixiPlatform {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PixiPlatform {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.name.cmp(&other.name)
    }
}

impl PartialEq for PixiPlatform {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for PixiPlatform {}

impl Hash for PixiPlatform {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl fmt::Display for PixiPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name.0.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_conda_types::{PackageName, Version};

    use super::*;

    fn rich(name: &str, subdir: Platform, vps: Vec<GenericVirtualPackage>) -> PixiPlatform {
        PixiPlatform::new(
            PixiPlatformName::try_from(name).expect("valid name"),
            subdir,
            vps,
        )
        .expect("rich platform with name != subdir")
    }

    fn gvp(name: &str, version: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: PackageName::try_from(name).unwrap(),
            version: Version::from_str(version).unwrap(),
            build_string: String::new(),
        }
    }

    /// Extract `(name, version)` pairs for entries whose virtual-package
    /// name is in `names`. Used by the apply-edit tests to assert what
    /// happened to the entries the test specifically operates on, without
    /// having to spell out every materialised subdir default.
    fn declared_by_name(platform: &PixiPlatform, names: &[&str]) -> Vec<(String, String)> {
        platform
            .declared_virtual_packages()
            .iter()
            .filter(|gvp| names.contains(&gvp.name.as_normalized()))
            .map(|gvp| {
                (
                    gvp.name.as_normalized().to_string(),
                    gvp.version.to_string(),
                )
            })
            .collect()
    }

    /// Returns `true` if the platform declares any virtual package whose
    /// name matches `name`. The default-merging policy guarantees the
    /// subdir defaults are always present on a rich platform, so the
    /// apply-edit tests use this to assert that removing a non-default
    /// VP actually strips it while removing a default-named VP gets
    /// re-seeded from the subdir defaults.
    fn declares(platform: &PixiPlatform, name: &str) -> bool {
        platform
            .declared_virtual_packages()
            .iter()
            .any(|gvp| gvp.name.as_normalized() == name)
    }

    #[test]
    fn apply_edit_upserts_replace_by_name() {
        let mut p = rich(
            "gpu-linux",
            Platform::Linux64,
            vec![gvp("__cuda", "12.0"), gvp("__glibc", "2.28")],
        );

        p.apply_edit(PlatformEdit {
            insert_or_update_virtual_packages: vec![gvp("__cuda", "12.4")],
            ..Default::default()
        })
        .unwrap();

        assert_eq!(
            declared_by_name(&p, &["__cuda", "__glibc"]),
            vec![
                ("__cuda".to_string(), "12.4".to_string()),
                ("__glibc".to_string(), "2.28".to_string()),
            ],
        );
    }

    #[test]
    fn apply_edit_clear_then_upsert_drops_old() {
        let mut p = rich(
            "gpu-linux",
            Platform::Linux64,
            vec![gvp("__cuda", "12.0"), gvp("__glibc", "2.28")],
        );

        p.apply_edit(PlatformEdit {
            clear_virtual_packages: true,
            insert_or_update_virtual_packages: vec![gvp("__archspec", "0")],
            ..Default::default()
        })
        .unwrap();

        // The clear+upsert drops the user's `__cuda`; `__glibc` is a subdir
        // default for linux-64 so it's re-seeded by the merge.
        assert!(!declares(&p, "__cuda"));
        assert!(declares(&p, "__archspec"));
        assert!(declares(&p, "__glibc"));
    }

    #[test]
    fn apply_edit_remove_by_name_removes_only_matches() {
        let mut p = rich(
            "gpu-linux",
            Platform::Linux64,
            vec![gvp("__cuda", "12.0"), gvp("__glibc", "2.28")],
        );

        p.apply_edit(PlatformEdit {
            remove_virtual_packages: vec![PackageName::try_from("__cuda").unwrap()],
            ..Default::default()
        })
        .unwrap();

        // `__cuda` has no subdir default, so the remove sticks; `__glibc`
        // would survive the remove regardless because the default-merge
        // re-seeds it.
        assert!(!declares(&p, "__cuda"));
        assert!(declares(&p, "__glibc"));
    }

    #[test]
    fn apply_edit_adding_vp_to_subdir_platform_renames_it() {
        // Adding a VP to a bare subdir-platform must move it off the subdir
        // name: a platform with VPs can never be named after its subdir.
        let mut p = PixiPlatform::from_subdir(Platform::Linux64);
        assert!(p.is_subdir_platform());
        p.apply_edit(PlatformEdit {
            insert_or_update_virtual_packages: vec![gvp("__cuda", "12.0")],
            ..Default::default()
        })
        .unwrap();
        assert!(!p.is_subdir_platform());
        assert_eq!(p.name().as_str(), "linux-64-cuda-12-0");
        assert_eq!(p.subdir(), Platform::Linux64);
    }

    /// Combine all four ops in one `PlatformEdit`. Documented order is
    /// clear → insert-or-update → remove → set_subdir, so the result is
    /// independent of how the caller orders the fields.
    #[test]
    fn apply_edit_combined_runs_in_documented_order() {
        let mut p = rich(
            "gpu-linux",
            Platform::Linux64,
            vec![
                gvp("__cuda", "11.0"),
                gvp("__archspec", "0"),
                gvp("__glibc", "2.17"),
            ],
        );

        p.apply_edit(PlatformEdit {
            clear_virtual_packages: false,
            insert_or_update_virtual_packages: vec![
                // Existing entry -> update in place.
                gvp("__cuda", "12.4"),
                // New entry -> push.
                gvp("__future_pkg", "1.2"),
            ],
            remove_virtual_packages: vec![
                PackageName::try_from("__archspec").unwrap(),
                // Removing a name we just inserted-or-updated must still
                // win because remove runs after the insert-or-update pass.
                PackageName::try_from("__future_pkg").unwrap(),
            ],
            set_subdir: Some(Platform::LinuxAarch64),
        })
        .unwrap();

        // `__cuda` was updated, `__future_pkg` is gone, and the user's
        // `__glibc = "2.17"` survives because it pre-empts the subdir
        // default. `__archspec` is a subdir default for linux-aarch64, so
        // even though the edit removed it, the merge re-seeds it.
        assert_eq!(
            declared_by_name(&p, &["__cuda", "__glibc"]),
            vec![
                ("__cuda".to_string(), "12.4".to_string()),
                ("__glibc".to_string(), "2.17".to_string()),
            ],
        );
        assert!(!declares(&p, "__future_pkg"));
        assert!(declares(&p, "__archspec"));
        assert_eq!(p.subdir(), Platform::LinuxAarch64);
        assert_eq!(p.name().as_str(), "gpu-linux");
    }

    #[test]
    fn apply_edit_dropping_last_vp_collapses_to_subdir() {
        // Removing the only non-default VP collapses the rich platform back
        // to a subdir platform. The subdir defaults are re-materialised so
        // the result is identical to `PixiPlatform::from_subdir`.
        let mut p = rich("gpu-linux", Platform::Linux64, vec![gvp("__cuda", "12.0")]);
        p.apply_edit(PlatformEdit {
            remove_virtual_packages: vec![PackageName::try_from("__cuda").unwrap()],
            ..Default::default()
        })
        .unwrap();
        assert!(p.is_subdir_platform());
        assert_eq!(p.name().as_str(), "linux-64");
        assert_eq!(
            p.declared_virtual_packages(),
            PixiPlatform::from_subdir(Platform::Linux64).declared_virtual_packages(),
        );
    }

    #[test]
    fn apply_edit_rejected_when_subdir_starts_matching_name() {
        // Renaming the subdir onto the platform's own name while it still
        // carries virtual packages would forge an illegal subdir-platform.
        let mut p = rich("linux-64", Platform::Win64, vec![gvp("__cuda", "12.0")]);
        let err = p
            .apply_edit(PlatformEdit {
                set_subdir: Some(Platform::Linux64),
                ..Default::default()
            })
            .unwrap_err();
        assert!(matches!(err, PixiPlatformError::IsSubdirPlatform));
    }

    #[test]
    fn apply_edit_set_subdir_changes_only_subdir() {
        let mut p = rich("gpu-linux", Platform::Linux64, vec![gvp("__cuda", "12.0")]);
        p.apply_edit(PlatformEdit {
            set_subdir: Some(Platform::LinuxAarch64),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(p.subdir(), Platform::LinuxAarch64);
        assert_eq!(p.name().as_str(), "gpu-linux");
        // `__cuda` is preserved verbatim, and the linux-aarch64 defaults are
        // materialised on top: `__unix`, `__linux`, `__glibc`, `__archspec`.
        assert_eq!(
            declared_by_name(&p, &["__cuda"]),
            vec![("__cuda".to_string(), "12.0".to_string())],
        );
        assert!(declares(&p, "__unix"));
        assert!(declares(&p, "__linux"));
        assert!(declares(&p, "__glibc"));
        assert!(declares(&p, "__archspec"));
    }

    #[test]
    fn set_name_rejected_on_subdir_platform() {
        // A bare subdir-platform is an alias for its subdir and can't be renamed.
        let mut p = PixiPlatform::from_subdir(Platform::Linux64);
        let err = p
            .set_name(PixiPlatformName::try_from("custom").unwrap())
            .unwrap_err();
        assert!(matches!(err, PixiPlatformError::IsSubdirPlatform));
    }

    #[test]
    fn set_name_to_subdir_name_rejected_when_vps_present() {
        // A VP-bearing platform may not be renamed onto its own subdir name.
        let mut p = rich("gpu", Platform::Linux64, vec![gvp("__cuda", "12.0")]);
        let err = p
            .set_name(PixiPlatformName::try_from("linux-64").unwrap())
            .unwrap_err();
        assert!(matches!(err, PixiPlatformError::IsSubdirPlatform));
    }

    #[test]
    fn name_rejects_reserved_family_names() {
        for reserved in ["linux", "unix", "win", "osx", "macos"] {
            let err = PixiPlatformName::try_from(reserved).unwrap_err();
            assert!(
                matches!(err, PixiPlatformNameError::ReservedName(ref n) if n == reserved),
                "expected ReservedName({reserved}), got {err:?}",
            );
        }
    }

    #[test]
    fn name_rejects_empty() {
        let err = PixiPlatformName::try_from("").unwrap_err();
        assert!(
            matches!(err, PixiPlatformNameError::Empty),
            "expected Empty, got {err:?}",
        );
    }

    #[test]
    fn name_rejects_invalid_characters() {
        let cases: &[(&str, char, usize)] = &[
            ("1linux", '1', 0),
            ("-linux", '-', 0),
            ("_linux", '_', 0),
            ("linux 64", ' ', 5),
            ("linux/64", '/', 5),
            ("linux.64", '.', 5),
            ("linux@64", '@', 5),
            ("linux+64", '+', 5),
            // Tab is a control byte; the validator renders it as U+FFFD.
            ("linux\t64", '\u{FFFD}', 5),
            // Trailing `-` was silently accepted before -- name must end
            // in an alphanumeric character.
            ("linux-", '-', 5),
            ("a-", '-', 1),
            ("ab-c-", '-', 4),
        ];
        for (input, expected_char, expected_pos) in cases {
            let err = PixiPlatformName::try_from(*input)
                .err()
                .unwrap_or_else(|| panic!("expected error for '{input}'"));
            assert!(
                matches!(
                    err,
                    PixiPlatformNameError::InvalidCharacter { character, position }
                        if character == *expected_char && position == *expected_pos
                ),
                "input '{input}': expected InvalidCharacter({expected_char:?}, {expected_pos}), got {err:?}",
            );
        }
    }

    #[test]
    fn name_rejects_too_long() {
        let long = "a".repeat(128);
        let err = PixiPlatformName::try_from(long.as_str()).unwrap_err();
        assert!(
            matches!(err, PixiPlatformNameError::TooLong { .. }),
            "expected TooLong, got {err:?}",
        );
    }

    /// A name-equals-subdir entry carrying virtual packages has no on-disk
    /// shape and would alias the bare subdir target selector, so
    /// `PixiPlatform::new` must reject it for every conda subdir; the only
    /// legal way to build `name == subdir` is via `from_subdir`. This is the
    /// invariant the legacy-sysreqs migration relies on.
    #[test]
    fn new_rejects_subdir_name_with_arbitrary_virtual_packages() {
        use strum::IntoEnumIterator;

        for subdir in Platform::iter() {
            if subdir == Platform::NoArch || subdir == Platform::Unknown {
                continue;
            }
            let name = PixiPlatformName::try_from(subdir.as_str()).unwrap_or_else(|e| {
                panic!(
                    "rattler subdir '{}' must be a valid name: {e:?}",
                    subdir.as_str()
                )
            });
            // A subdir-named entry with a virtual package that isn't a
            // subdir default has no on-disk shape and must be rejected.
            let err =
                PixiPlatform::new(name.clone(), subdir, vec![gvp("__cuda", "12.0")]).unwrap_err();
            assert!(
                matches!(err, PixiPlatformError::IsSubdirPlatform),
                "subdir '{}' + non-default VPs should be rejected, got {err:?}",
                subdir.as_str(),
            );

            // Exactly the subdir defaults (which is the only legal shape
            // for a subdir-named entry) must succeed.
            PixiPlatform::new(
                name.clone(),
                subdir,
                subdir_default_virtual_packages(subdir),
            )
            .unwrap_or_else(|e| {
                panic!(
                    "subdir '{}' + subdir defaults must construct: {e:?}",
                    subdir.as_str()
                )
            });
        }
    }

    /// Family selectors (`linux`/`unix`/`win`/`osx`/`macos`) double as
    /// `target.<family>.*` keys; a `PixiPlatformName` carrying any of them
    /// would shadow that selector. The name validator must reject them
    /// before we ever get a chance to call `PixiPlatform::new` with such a
    /// name, so no rich entry can end up family-named.
    #[test]
    fn name_validator_blocks_target_selector_family_names() {
        for family in ["linux", "unix", "win", "osx", "macos"] {
            let err = PixiPlatformName::try_from(family).unwrap_err();
            assert!(
                matches!(err, PixiPlatformNameError::ReservedName(ref n) if n == family),
                "family '{family}' should be rejected as ReservedName, got {err:?}",
            );
            assert!(
                crate::target::family_name_to_selector(family).is_some(),
                "family '{family}' must round-trip to a TargetSelector",
            );
        }
    }

    /// Every real conda subdir must round-trip through `PixiPlatformName`
    /// and `from_subdir` must produce a locked-down subdir-platform that
    /// carries exactly the subdir defaults.
    #[test]
    fn every_rattler_platform_round_trips_and_is_locked() {
        use strum::IntoEnumIterator;

        for subdir in Platform::iter() {
            if subdir == Platform::NoArch || subdir == Platform::Unknown {
                continue;
            }

            let subdir_str = subdir.as_str();

            let parsed = PixiPlatformName::try_from(subdir_str).unwrap_or_else(|e| {
                panic!("rattler subdir '{subdir_str}' rejected by validator: {e:?}")
            });
            assert_eq!(parsed.as_str(), subdir_str);

            let via_from: PixiPlatformName = subdir.into();
            assert_eq!(via_from, parsed);

            let mut platform = PixiPlatform::from_subdir(subdir);
            assert!(
                platform.is_subdir_platform(),
                "from_subdir({subdir_str}) should be a subdir-platform",
            );
            assert_eq!(platform.name().as_str(), subdir_str);
            assert_eq!(platform.subdir(), subdir);
            assert_eq!(
                platform.declared_virtual_packages(),
                subdir_default_virtual_packages(subdir).as_slice(),
                "from_subdir({subdir_str}) must materialise the subdir defaults",
            );

            let some_other_subdir = if subdir == Platform::Linux64 {
                Platform::Osx64
            } else {
                Platform::Linux64
            };
            let alt_name = PixiPlatformName::try_from("custom").unwrap();
            assert!(matches!(
                platform.set_name(alt_name),
                Err(PixiPlatformError::IsSubdirPlatform)
            ));
            assert!(matches!(
                platform.set_subdir(some_other_subdir),
                Err(PixiPlatformError::IsSubdirPlatform)
            ));
            assert!(matches!(
                platform.set_declared_virtual_packages(vec![gvp("__cuda", "12.0")]),
                Err(PixiPlatformError::IsSubdirPlatform)
            ));
            // set_subdir-only edits are blocked: the subdir baseline is
            // immutable, so changing the subdir on a subdir platform is
            // not allowed (the caller must build a new subdir platform).
            assert!(matches!(
                platform.apply_edit(PlatformEdit {
                    set_subdir: Some(some_other_subdir),
                    ..Default::default()
                }),
                Err(PixiPlatformError::IsSubdirPlatform)
            ));
            // Clearing the materialised defaults is also blocked.
            assert!(matches!(
                platform.apply_edit(PlatformEdit {
                    clear_virtual_packages: true,
                    ..Default::default()
                }),
                Err(PixiPlatformError::IsSubdirPlatform)
            ));
            // Removing a default by name is a no-op after the re-merge,
            // so the edit is rejected as "nothing changed".
            assert!(matches!(
                platform.apply_edit(PlatformEdit {
                    remove_virtual_packages: vec![PackageName::try_from("__linux").unwrap(),],
                    ..Default::default()
                }),
                Err(PixiPlatformError::IsSubdirPlatform)
            ));
        }
    }

    /// A subdir platform can only be transformed by adding a virtual package
    /// whose value differs from the subdir defaults. That transition turns
    /// it into a rich platform with a synthesised name.
    #[test]
    fn apply_edit_transitions_subdir_platform_to_rich() {
        let mut p = PixiPlatform::from_subdir(Platform::Linux64);
        assert!(p.is_subdir_platform());
        p.apply_edit(PlatformEdit {
            insert_or_update_virtual_packages: vec![gvp("__cuda", "12.0")],
            ..Default::default()
        })
        .unwrap();
        assert!(!p.is_subdir_platform());
        assert_eq!(p.name().as_str(), "linux-64-cuda-12-0");
        assert_eq!(p.subdir(), Platform::Linux64);
        // The defaults survive the transition.
        assert!(declares(&p, "__linux"));
        assert!(declares(&p, "__glibc"));
        assert!(declares(&p, "__archspec"));
    }
}
