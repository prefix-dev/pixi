use std::fmt::{self, Display, Formatter};
use std::hash::{Hash, Hasher};
use std::ops::Deref;

use rattler_conda_types::{GenericVirtualPackage, PackageName, Platform};
use rattler_virtual_packages::{
    DetectVirtualPackageError, Override, VirtualPackageOverrides, VirtualPackages,
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
    /// Build a `PixiPlatform` from a bare subdir. The name is set to the
    /// subdir string and the virtual-package set is empty.
    pub fn from_subdir(subdir: Platform) -> Self {
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
    /// `name == subdir`) can never carry virtual packages: such an entry has
    /// no syntactic shape in `pixi.toml` (bare-string form omits VPs) and the
    /// target-selector machinery treats it as the bare subdir alias.
    pub fn new(
        name: PixiPlatformName,
        subdir: Platform,
        declared_virtual_packages: Vec<GenericVirtualPackage>,
    ) -> Result<Self, PixiPlatformError> {
        if name.as_str() == subdir.as_str() && !declared_virtual_packages.is_empty() {
            return Err(PixiPlatformError::IsSubdirPlatform);
        }
        Ok(Self {
            name,
            subdir,
            declared_virtual_packages,
        })
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
    /// The name then follows the subdir-platform invariant -- a platform
    /// without virtual packages is always named after its subdir:
    /// - a bare subdir-platform may only be edited into a richer one; an edit
    ///   that would leave it without virtual packages is rejected with
    ///   [`PixiPlatformError::IsSubdirPlatform`];
    /// - adding a VP to a bare subdir-platform, or any edit of an auto-named
    ///   platform, recomputes the synthesised name;
    /// - dropping the last VP of a rich platform resets the name to the subdir;
    /// - a custom name is preserved across VP edits, but
    ///   [`PixiPlatformError::IsSubdirPlatform`] is returned if the edit would
    ///   leave a VP-bearing platform named after its own subdir.
    pub fn apply_edit(&mut self, edit: PlatformEdit) -> Result<(), PixiPlatformError> {
        let was_subdir = self.is_subdir_platform();
        // A bare subdir-platform and a synthesised platform both carry the
        // auto-derived name; only an explicit custom name differs from it, and
        // such a name is preserved across the edit.
        let was_auto = self.name.as_str()
            == crate::toml::platform::synthesize_name_string(
                self.subdir,
                &self.declared_virtual_packages,
            );

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

        if self.declared_virtual_packages.is_empty() {
            if was_subdir {
                // A bare subdir-platform can only be edited into a richer one;
                // an edit that leaves it bare is rejected to keep bare entries
                // pristine.
                return Err(PixiPlatformError::IsSubdirPlatform);
            }
            // Dropping the last virtual package collapses a rich platform back
            // to a bare subdir-platform: the name must equal the subdir.
            self.name = self.subdir.into();
        } else if was_auto {
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
            p.declared_virtual_packages()
                .iter()
                .map(|g| (g.name.as_normalized().to_string(), g.version.to_string()))
                .collect::<Vec<_>>(),
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

        assert_eq!(p.declared_virtual_packages().len(), 1);
        assert_eq!(
            p.declared_virtual_packages()[0].name.as_normalized(),
            "__archspec"
        );
    }

    #[test]
    fn apply_edit_remove_by_name_removes_only_matches() {
        let mut p = rich(
            "gpu-linux",
            Platform::Linux64,
            vec![gvp("__cuda", "12.0"), gvp("__glibc", "2.28")],
        );

        p.apply_edit(PlatformEdit {
            remove_virtual_packages: vec![PackageName::try_from("__glibc").unwrap()],
            ..Default::default()
        })
        .unwrap();

        assert_eq!(p.declared_virtual_packages().len(), 1);
        assert_eq!(
            p.declared_virtual_packages()[0].name.as_normalized(),
            "__cuda"
        );
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

        // __cuda was updated, __archspec and __future_pkg are gone,
        // __glibc was untouched.
        let kept: Vec<_> = p
            .declared_virtual_packages()
            .iter()
            .map(|g| (g.name.as_normalized().to_string(), g.version.to_string()))
            .collect();
        assert_eq!(
            kept,
            vec![
                ("__cuda".to_string(), "12.4".to_string()),
                ("__glibc".to_string(), "2.17".to_string()),
            ],
        );
        assert_eq!(p.subdir(), Platform::LinuxAarch64);
        assert_eq!(p.name().as_str(), "gpu-linux");
    }

    #[test]
    fn apply_edit_dropping_last_vp_collapses_to_subdir() {
        // Removing the only virtual package leaves nothing to distinguish the
        // platform from its subdir, so it must become a bare subdir-platform.
        let mut p = rich("gpu-linux", Platform::Linux64, vec![gvp("__cuda", "12.0")]);
        p.apply_edit(PlatformEdit {
            remove_virtual_packages: vec![PackageName::try_from("__cuda").unwrap()],
            ..Default::default()
        })
        .unwrap();
        assert!(p.is_subdir_platform());
        assert_eq!(p.name().as_str(), "linux-64");
        assert!(p.declared_virtual_packages().is_empty());
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
        assert_eq!(p.declared_virtual_packages().len(), 1);
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
    fn new_rejects_subdir_name_with_virtual_packages() {
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
            let err = PixiPlatform::new(name, subdir, vec![gvp("__cuda", "12.0")]).unwrap_err();
            assert!(
                matches!(err, PixiPlatformError::IsSubdirPlatform),
                "subdir '{}' + VPs should be rejected as IsSubdirPlatform, got {err:?}",
                subdir.as_str(),
            );
        }

        // Empty VP list is the valid bare-subdir construction and must succeed.
        for subdir in Platform::iter() {
            if subdir == Platform::NoArch || subdir == Platform::Unknown {
                continue;
            }
            let name = PixiPlatformName::try_from(subdir.as_str()).unwrap();
            PixiPlatform::new(name, subdir, Vec::new()).unwrap_or_else(|e| {
                panic!("bare subdir '{}' must construct: {e:?}", subdir.as_str())
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
    /// and `from_subdir` must produce a locked-down subdir-platform.
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
            assert!(platform.declared_virtual_packages().is_empty());

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
            assert!(matches!(
                platform.apply_edit(PlatformEdit {
                    set_subdir: Some(some_other_subdir),
                    ..Default::default()
                }),
                Err(PixiPlatformError::IsSubdirPlatform)
            ));
        }
    }
}
