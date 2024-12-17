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

use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use toml_span::de_helpers::expected;
use toml_span::{value::ValueInner, DeserError, Value};

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

impl<'de> toml_span::Deserialize<'de> for Preview {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::Boolean(value) => Ok(Preview::AllEnabled(value)),
            ValueInner::Array(arr) => {
                let features = arr
                    .into_iter()
                    .map(|mut value| toml_span::Deserialize::deserialize(&mut value))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Preview::Features(features))
            }
            other => Err(DeserError::from(expected(
                "bool or list of features e.g `true` or `[\"new-resolve\"]`",
                other,
                value.span,
            ))),
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for PreviewFeature {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let str = value.take_string("a feature name".into())?;
        Ok(KnownPreviewFeature::from_str(&str).map_or_else(
            |_| PreviewFeature::Unknown(str.into_owned()),
            PreviewFeature::Known,
        ))
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

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use toml_span::de_helpers::TableHelper;

    use super::*;
    use crate::{toml::FromTomlStr, utils::test_utils::format_parse_error};

    /// Fake table to test the `Preview` enum
    #[derive(Debug)]
    struct TopLevel {
        preview: Preview,
    }

    impl<'de> toml_span::Deserialize<'de> for TopLevel {
        fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
            let mut th = TableHelper::new(value)?;
            let preview = th.required("preview")?;
            th.finalize(None)?;
            Ok(TopLevel { preview })
        }
    }

    #[test]
    fn test_preview_all_enabled() {
        let input = "preview = true";
        let top = TopLevel::from_toml_str(input).expect("should parse as `AllEnabled`");
        assert_eq!(top.preview, Preview::AllEnabled(true));
    }

    #[test]
    fn test_preview_with_unknown_feature() {
        let input = r#"preview = ["build"]"#;
        let top =
            TopLevel::from_toml_str(input).expect("should parse as `Features` with known feature");
        assert_eq!(
            top.preview,
            Preview::Features(vec![PreviewFeature::Unknown("build".to_string())])
        );
    }

    #[test]
    fn test_insta_error_invalid_bool() {
        let input = r#"preview = "not-a-bool""#;
        let result = TopLevel::from_toml_str(input);

        assert_snapshot!(
            format_parse_error(input, result.unwrap_err()),
            @r###"
         × expected bool or list of features e.g `true` or `["new-resolve"]`, found string
          ╭─[pixi.toml:1:12]
        1 │ preview = "not-a-bool"
          ·            ──────────
          ╰────
        "###
        );
    }

    #[test]
    fn test_insta_error_invalid_list_item() {
        let input = r#"preview = ["build", 123]"#;
        let result = TopLevel::from_toml_str(input);

        assert!(result.is_err());
        assert_snapshot!(
            format_parse_error(input, result.unwrap_err()),
            @r###"
         × expected a feature name, found integer
          ╭─[pixi.toml:1:21]
        1 │ preview = ["build", 123]
          ·                     ───
          ╰────
        "###
        );
    }

    #[test]
    fn test_insta_error_invalid_top_level_type() {
        let input = r#"preview = 123"#;
        let result = TopLevel::from_toml_str(input);

        assert!(result.is_err());
        assert_snapshot!(
            format_parse_error(input, result.unwrap_err()),
            @r###"
         × expected bool or list of features e.g `true` or `["new-resolve"]`, found integer
          ╭─[pixi.toml:1:11]
        1 │ preview = 123
          ·           ───
          ╰────
        "###
        );
    }

    #[test]
    fn test_feature_is_unknown() {
        let input = r#"preview = ["new_parsing"]"#;
        let top = TopLevel::from_toml_str(input).unwrap();
        match top.preview {
            Preview::AllEnabled(_) => unreachable!("this arm should not be used"),
            Preview::Features(vec) => {
                assert_matches::assert_matches!(
                    &vec[0],
                    PreviewFeature::Unknown(s) => {
                        s == &"new_parsing".to_string()
                    }
                );
            }
        }
    }
}
