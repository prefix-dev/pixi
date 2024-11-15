use serde::de::{self};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum Preview {
    /// All preview features are enabled
    AllEnabled(bool), // For `preview = true`
    /// Specific preview features are enabled
    Features(Vec<PreviewFeature>), // For `preview = ["feature"]`
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
pub enum PreviewFeature {
    /// This is a known preview feature
    Known(KnownFeature),
    /// Unknown preview feature
    Unknown(String), // Catch-all for unknown strings
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum KnownFeature {
    // Add known features here
}

impl<'de> Deserialize<'de> for PreviewFeature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_value::Value::deserialize(deserializer)?;
        let known = KnownFeature::deserialize(value.clone()).map(PreviewFeature::Known);
        if let Ok(feature) = known {
            Ok(feature)
        } else {
            let unknown = String::deserialize(value)
                .map(PreviewFeature::Unknown)
                .map_err(de::Error::custom)?;
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

        let output = to_string(&preview).expect("should serialize back to TOML");
        assert_eq!(output.trim(), input);
    }

    #[test]
    fn test_insta_error_invalid_bool() {
        let input = r#"preview = "not-a-bool""#;
        let result: Result<Preview, _> = from_str(input);

        assert!(result.is_err());
        assert_snapshot!(
            format!("{:?}", result.unwrap_err()),
            @r###"
            "###
        );
    }

    #[test]
    fn test_insta_error_invalid_list_item() {
        let input = r#"preview = ["build", 123]"#;
        let result: Result<Preview, _> = from_str(input);

        assert!(result.is_err());
        assert_snapshot!(
            format!("{:?}", result.unwrap_err()),
            @r###"
            "###
        );
    }

    #[test]
    fn test_insta_error_invalid_top_level_type() {
        let input = r#"preview = 123"#;
        let result: Result<Preview, _> = from_str(input);

        assert!(result.is_err());
        assert_snapshot!(
            format!("{:?}", result.unwrap_err()),
            @r###"
            "###
        );
    }
}
