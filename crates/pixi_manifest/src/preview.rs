//! This module contains the ability to parse the preview features of the
//! project
//!
//! e.g.
//! ```toml
//! [project]
//! # .. other project metadata
//! preview = ["new-resolve"]
//! ```
//!
//! Features are split into Known and Unknown features. Basically you can use
//! any string as a feature but only the features defined in [`KnownFeature`]
//! can be used. We do this for backwards compatibility with the old features
//! that may have been used in the past. The [`KnownFeature`] enum contains all
//! the known features. Extend this if you want to add support for new features.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
/// The preview features of the project
pub enum Preview {
    /// All preview features are enabled
    AllEnabled(bool), // For `preview = true`
    /// Specific preview features are enabled
    Features(Vec<KnownPreviewFeature>), // For `preview = ["feature"]`
}

impl Default for Preview {
    fn default() -> Self {
        Self::Features(Vec::new())
    }
}

impl Preview {
    /// Returns true if all preview features are enabled
    pub fn all_enabled(&self) -> bool {
        match self {
            Preview::AllEnabled(enabled) => *enabled,
            Preview::Features(_) => false,
        }
    }

    /// Returns true if the given preview feature is enabled
    pub fn is_enabled(&self, feature: KnownPreviewFeature) -> bool {
        match self {
            Preview::AllEnabled(_) => true,
            Preview::Features(features) => features.iter().any(|f| *f == feature),
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
    strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
/// Currently supported preview features are listed here
pub enum KnownPreviewFeature {
    /// Build feature, to enable conda source builds
    PixiBuild,
}
