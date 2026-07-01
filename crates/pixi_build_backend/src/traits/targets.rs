//! Targets behaviour traits.
//!
//! # Key components
//!
//! * [`Targets`] - A project target trait.
//! * [`TargetSelector`] - An extension trait that extends the target selector with additional functionality.
use rattler_conda_types::Platform;

use crate::PackageSpec;
use pixi_build_types::{self as pbt};

/// A trait that extend the target selector with additional functionality.
pub trait TargetSelector {
    /// Does the target selector match the platform?
    fn matches(&self, platform: Platform) -> bool;
}

/// A trait that represent a project target.
///
/// Dependencies are carried on the default target plus conditional
/// `if(<expression>)` entries. They are only converted into recipe
/// requirements and evaluated by rattler-build; backends never inspect
/// them, so this trait exposes no dependency accessors.
pub trait Targets {
    /// The target it is resolving to
    type Target;

    /// The Spec type that is used in the package spec
    type Spec: PackageSpec;

    /// Returns the default target.
    fn default_target(&self) -> Option<&Self::Target>;

    /// Return a spec that matches any version
    fn empty_spec() -> Self::Spec;
}

// === Below here are the implementations for v1 ===
impl TargetSelector for pbt::TargetSelector {
    fn matches(&self, platform: Platform) -> bool {
        match self {
            pbt::TargetSelector::Platform(p) => p == &platform.to_string(),
            pbt::TargetSelector::Subdir(s) => s == &platform.to_string(),
            pbt::TargetSelector::Linux => platform.is_linux(),
            pbt::TargetSelector::Unix => platform.is_unix(),
            pbt::TargetSelector::Win => platform.is_windows(),
            pbt::TargetSelector::MacOs => platform.is_osx(),
        }
    }
}

impl Targets for pbt::Targets {
    type Target = pbt::Target;

    type Spec = pbt::PackageSpec;

    fn default_target(&self) -> Option<&pbt::Target> {
        self.default_target.as_ref()
    }

    fn empty_spec() -> pbt::PackageSpec {
        rattler_conda_types::VersionSpec::Any.into()
    }
}
