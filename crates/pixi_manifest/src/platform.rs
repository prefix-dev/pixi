use std::fmt::{self, Display, Formatter};
use std::hash::{Hash, Hasher};
use std::ops::Deref;

use rattler_conda_types::{GenericVirtualPackage, Platform};
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
}

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
        for (pos, c) in bytes.iter().enumerate() {
            let character = if !c.is_ascii_control() && *c < 128 {
                *c as char
            } else {
                '�'
            };

            match (pos, c) {
                (0, c) if !c.is_ascii_lowercase() && !c.is_ascii_alphabetic() => {
                    return Err(PixiPlatformNameError::InvalidCharacter {
                        character,
                        position: 0,
                    });
                }
                (p, c) if p == input_len && !c.is_ascii_alphanumeric() => {
                    return Err(PixiPlatformNameError::InvalidCharacter {
                        character,
                        position: p,
                    });
                }
                (p, c) if p < input_len && !c.is_ascii_alphanumeric() && *c != b'-' => {
                    return Err(PixiPlatformNameError::InvalidCharacter {
                        character,
                        position: p,
                    });
                }
                _ => {}
            };
        }
        Ok(input.to_string())
    }
}

/// Family-style target selectors that must not be usable as workspace platform
/// names -- the manifest reads them as keys for `target.unix.*` etc., so a
/// platform literally called `unix` would collide.
const RESERVED_FAMILY_NAMES: &[&str] = &["linux", "unix", "win", "osx"];

impl TryFrom<&str> for PixiPlatformName {
    type Error = PixiPlatformNameError;

    fn try_from(input: &str) -> Result<Self, Self::Error> {
        let validated = Self::valid_pixi_platform_name(input)?;
        if RESERVED_FAMILY_NAMES.contains(&validated.as_str()) {
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
        if self.is_subdir_platform() {
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

    /// Build a new `PixiPlatform`
    pub fn new(
        name: PixiPlatformName,
        subdir: Platform,
        declared_virtual_packages: Vec<GenericVirtualPackage>,
    ) -> Self {
        Self {
            name,
            subdir,
            declared_virtual_packages,
        }
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
