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

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq)]
/// The preview features of the project
pub enum Preview {
    /// All preview features are enabled
    AllEnabled(bool), // For `preview = true`
    /// Specific preview features are enabled
    Features(Vec<PreviewFeature>), // For `preview = ["feature"]`
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

    /// Return all unknown preview features
    pub fn unknown_preview_features(&self) -> Vec<&str> {
        match self {
            Preview::AllEnabled(_) => vec![],
            Preview::Features(features) => features
                .iter()
                .filter_map(|feature| match feature {
                    PreviewFeature::Unknown(feature) => Some(feature.as_str()),
                    _ => None,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(untagged)]
/// A preview feature, can be either a known feature or an unknown feature
pub enum PreviewFeature {
    /// This is a known preview feature
    Known(KnownPreviewFeature),
    /// Unknown preview feature
    Unknown(String),
}

impl PartialEq<KnownPreviewFeature> for PreviewFeature {
    fn eq(&self, other: &KnownPreviewFeature) -> bool {
        match self {
            PreviewFeature::Known(feature) => feature == other,
            _ => false,
        }
    }
}

#[derive(
    Debug, Serialize, Deserialize, Clone, Copy, PartialEq, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
/// Currently supported preview features are listed here
pub enum KnownPreviewFeature {
    /// Build feature, to enable conda source builds
    PixiBuild,
}

impl<'de> Deserialize<'de> for PreviewFeature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_value::Value::deserialize(deserializer)?;
        let known = KnownPreviewFeature::deserialize(value.clone()).map(PreviewFeature::Known);
        if let Ok(feature) = known {
            Ok(feature)
        } else {
            let unknown = String::deserialize(value)
                .map(PreviewFeature::Unknown)
                .map_err(serde::de::Error::custom)?;
            Ok(unknown)
        }
    }
}

impl KnownPreviewFeature {
    /// Returns the string representation of the feature
    pub fn as_str(&self) -> &'static str {
        match self {
            KnownPreviewFeature::PixiBuild => "pixi-build",
        }
    }
}
