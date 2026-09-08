//! This module contains the ability to parse the preview flags of the
//! project
//!
//! e.g.
//! ```toml
//! [project]
//! # .. other project metadata
//! preview = ["new-resolve"]
//! ```
//!
//! Flags are split into known and unknown flags. Basically you can use
//! any string as a flag but only the flags defined in [`KnownPreviewFlag`]
//! are used by pixi, unknown flags only produce a warning. We do this for
//! backwards compatibility with flags that may have been used in the past.
//! Extend the [`KnownPreviewFlag`] enum to add support for new flags.

use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

#[derive(Debug, Clone, PartialEq)]
/// The preview flags of the project
pub enum Preview {
    /// All preview flags are enabled
    AllEnabled(bool), // For `preview = true`
    /// Specific preview flags are enabled
    Flags(Vec<KnownPreviewFlag>), // For `preview = ["pixi-build"]`
}

impl Default for Preview {
    fn default() -> Self {
        Self::Flags(Vec::new())
    }
}

impl FromIterator<KnownPreviewFlag> for Preview {
    fn from_iter<T: IntoIterator<Item = KnownPreviewFlag>>(iter: T) -> Self {
        Self::Flags(iter.into_iter().collect())
    }
}

impl Preview {
    /// Returns true if all preview flags are enabled
    pub fn all_enabled(&self) -> bool {
        match self {
            Preview::AllEnabled(enabled) => *enabled,
            Preview::Flags(_) => false,
        }
    }

    /// Returns true if the given preview flag is enabled
    pub fn is_enabled(&self, flag: KnownPreviewFlag) -> bool {
        match self {
            Preview::AllEnabled(_) => true,
            Preview::Flags(flags) => flags.contains(&flag),
        }
    }

    /// Returns the enabled preview flags
    pub fn enabled_flags(&self) -> Vec<KnownPreviewFlag> {
        match self {
            Preview::AllEnabled(true) => KnownPreviewFlag::iter().collect(),
            Preview::AllEnabled(false) => Vec::new(),
            Preview::Flags(flags) => flags.clone(),
        }
    }
}

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    Copy,
    PartialEq,
    Eq,
    strum::Display,
    strum::EnumString,
    strum::EnumIter,
    strum::IntoStaticStr,
    strum::VariantNames,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
/// Currently supported preview flags are listed here
pub enum KnownPreviewFlag {
    /// Build flag, to enable conda source builds
    PixiBuild,
}
