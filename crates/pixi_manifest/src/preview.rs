//! This module contains the ability to parse the preview features of the project
//!
//! e.g.
//! ```toml
//! [project]
//! # .. other project metadata
//! preview = ["new-resolve"]
//! ```
//!
//! Features are split into Known and Unknown features. Basically you can use any string as a feature
//! but only the features defined in [`KnownFeature`] can be used.
//! We do this for backwards compatibility with the old features that may have been used in the past.
//! The [`KnownFeature`] enum contains all the known features. Extend this if you want to add support
//! for new features.
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(untagged)]
/// The preview features of the project
pub enum Preview {
    /// All preview features are enabled
    AllEnabled(bool), // For `preview = true`
    /// Specific preview features are enabled
    Features(Vec<PreviewFeature>), // For `preview = ["feature"]`
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

impl<'de> Deserialize<'de> for Preview {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .bool(|bool| Ok(Preview::AllEnabled(bool)))
            .seq(|seq| Ok(Preview::Features(seq.deserialize()?)))
            .expecting("bool or list of features e.g `true` or `[\"new-resolve\"]`")
            .deserialize(deserializer)
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

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "kebab-case")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;
    use toml_edit::{de::from_str, ser::to_string};

    /// Fake table to test the `Preview` enum
    #[derive(Debug, Serialize, Deserialize)]
    struct TopLevel {
        preview: Preview,
    }

    #[test]
    fn test_preview_all_enabled() {
        let input = "preview = true";
        let top: TopLevel = from_str(input).expect("should parse as `AllEnabled`");
        assert_eq!(top.preview, Preview::AllEnabled(true));

        let output = to_string(&top).expect("should serialize back to TOML");
        assert_eq!(output.trim(), input);
    }

    #[test]
    fn test_preview_with_unknown_feature() {
        let input = r#"preview = ["build"]"#;
        let top: TopLevel = from_str(input).expect("should parse as `Features` with known feature");
        assert_eq!(
            top.preview,
            Preview::Features(vec![PreviewFeature::Unknown("build".to_string())])
        );

        let output = to_string(&top).expect("should serialize back to TOML");
        assert_eq!(output.trim(), input);
    }

    #[test]
    fn test_insta_error_invalid_bool() {
        let input = r#"preview = "not-a-bool""#;
        let result: Result<Preview, _> = from_str(input);

        assert!(result.is_err());
        assert_snapshot!(
            format!("{:?}", result.unwrap_err()),
            @r###"Error { inner: TomlError { message: "invalid type: map, expected bool or list of features e.g `true` or `[\"new-resolve\"]`", raw: Some("preview = \"not-a-bool\""), keys: [], span: Some(0..22) } }"###
        );
    }

    #[test]
    fn test_insta_error_invalid_list_item() {
        let input = r#"preview = ["build", 123]"#;
        let result: Result<TopLevel, _> = from_str(input);

        assert!(result.is_err());
        assert_snapshot!(
            format!("{:?}", result.unwrap_err()),
            @r###"Error { inner: TomlError { message: "Invalid type integer `123`. Expected a string\n", raw: Some("preview = [\"build\", 123]"), keys: ["preview"], span: Some(10..24) } }"###
        );
    }

    #[test]
    fn test_insta_error_invalid_top_level_type() {
        let input = r#"preview = 123"#;
        let result: Result<TopLevel, _> = from_str(input);

        assert!(result.is_err());
        assert_snapshot!(
            format!("{:?}", result.unwrap_err()),
            @r###"Error { inner: TomlError { message: "invalid type: integer `123`, expected bool or list of features e.g `true` or `[\"new-resolve\"]`", raw: Some("preview = 123"), keys: ["preview"], span: Some(10..13) } }"###
        );
    }

    #[test]
    fn test_feature_is_unknown() {
        let input = r#"preview = ["new_parsing"]"#;
        let top: TopLevel = from_str(input).unwrap();
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
