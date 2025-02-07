use std::{ops::Range, str::FromStr};

use itertools::Itertools;
use miette::LabeledSpan;
use toml_span::{de_helpers::expected, value::ValueInner, DeserError, Spanned, Value};

use crate::{error::GenericError, KnownPreviewFeature, Preview, WithWarnings};

#[derive(Debug, Clone, PartialEq)]
/// The preview features of the project
pub enum TomlPreview {
    /// All preview features are enabled
    AllEnabled(Spanned<bool>), // For `preview = true`
    /// Specific preview features are enabled
    Features(Vec<Spanned<KnownOrUnknownPreviewFeature>>), // For `preview = ["feature"]`
}

impl Default for TomlPreview {
    fn default() -> Self {
        Self::Features(Vec::new())
    }
}

impl TomlPreview {
    /// Returns the span of the definition of a certain feature.
    pub fn get_span(&self, feature: KnownPreviewFeature) -> Option<Range<usize>> {
        match self {
            TomlPreview::AllEnabled(enabled) => enabled.value.then(|| enabled.span.into()),
            TomlPreview::Features(features) => features.iter().find_map(|f| {
                if f.value == KnownOrUnknownPreviewFeature::Known(feature) {
                    Some(f.span.into())
                } else {
                    None
                }
            }),
        }
    }

    /// Returns true if the given preview feature is enabled
    pub fn is_enabled(&self, feature: KnownPreviewFeature) -> bool {
        match self {
            Self::AllEnabled(_) => true,
            Self::Features(features) => features
                .iter()
                .any(|f| f.value == KnownOrUnknownPreviewFeature::Known(feature)),
        }
    }
}

impl TomlPreview {
    pub fn into_preview(self) -> WithWarnings<Preview> {
        match self {
            TomlPreview::AllEnabled(all_enabled) => {
                WithWarnings::from(Preview::AllEnabled(all_enabled.value))
            }
            TomlPreview::Features(features) => {
                let mut known_features = Vec::with_capacity(features.len());
                let mut unknown_features = Vec::new();
                for Spanned { value, span } in features {
                    match value {
                        KnownOrUnknownPreviewFeature::Known(feature) => {
                            known_features.push(feature)
                        }
                        KnownOrUnknownPreviewFeature::Unknown(feature) => {
                            unknown_features.push((feature, span))
                        }
                    };
                }
                let preview = WithWarnings::from(Preview::Features(known_features));
                if unknown_features.is_empty() {
                    preview
                } else {
                    let are = if unknown_features.len() > 1 {
                        "are"
                    } else {
                        "is"
                    };
                    let s = if unknown_features.len() > 1 { "s" } else { "" };
                    let warning = GenericError::new(
                        format!("The preview feature{s}: {} {are} defined in the manifest but un-used in pixi",
                                unknown_features.iter().map(|(name, _)| name).format(", ")))
                        .with_labels(unknown_features.into_iter().map(|(name, span)| {
                            LabeledSpan::new_with_span(Some(format!("'{}' is unknown", name)), Range::<usize>::from(span))
                        }));
                    preview.with_warnings(vec![warning.into()])
                }
            }
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlPreview {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let span = value.span;
        match value.take() {
            ValueInner::Boolean(enabled) => Ok(TomlPreview::AllEnabled(Spanned {
                value: enabled,
                span,
            })),
            ValueInner::Array(arr) => {
                let features = arr
                    .into_iter()
                    .map(|mut value| toml_span::Deserialize::deserialize(&mut value))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(TomlPreview::Features(features))
            }
            other => Err(DeserError::from(expected(
                "bool or list of features e.g `true` or `[\"new-resolve\"]`",
                other,
                value.span,
            ))),
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for KnownOrUnknownPreviewFeature {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let str = value.take_string("a feature name".into())?;
        Ok(KnownPreviewFeature::from_str(&str).map_or_else(
            |_| KnownOrUnknownPreviewFeature::Unknown(str.into_owned()),
            KnownOrUnknownPreviewFeature::Known,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnownOrUnknownPreviewFeature {
    Known(KnownPreviewFeature),
    Unknown(String),
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use insta::assert_snapshot;
    use toml_span::de_helpers::TableHelper;

    use super::*;
    use crate::{
        toml::{preview::KnownOrUnknownPreviewFeature::Unknown, FromTomlStr},
        utils::test_utils::format_parse_error,
    };

    /// Fake table to test the `Preview` enum
    #[derive(Debug)]
    struct TopLevel {
        preview: TomlPreview,
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
        assert_matches!(
            top.preview,
            TomlPreview::AllEnabled(Spanned { value: true, .. })
        );
    }

    #[test]
    fn test_preview_with_unknown_feature() {
        let input = r#"preview = ["build"]"#;
        let top =
            TopLevel::from_toml_str(input).expect("should parse as `Features` with known feature");
        match top.preview {
            TomlPreview::Features(vec) => {
                assert_eq!(vec[0].value, Unknown("build".to_string()));
            }
            _ => unreachable!("this arm should not be used"),
        }
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
            TomlPreview::AllEnabled(_) => unreachable!("this arm should not be used"),
            TomlPreview::Features(vec) => {
                assert_matches::assert_matches!(
                    &vec[0].value,
                    Unknown(s) => {
                        s == &"new_parsing".to_string()
                    }
                );
            }
        }
    }

    #[test]
    fn test_unknown_feature_warning() {
        let input = r#"preview = ["foobar", "pixi-build", "new_parsing"]"#;
        let top = TopLevel::from_toml_str(input).unwrap();
        let preview = top.preview.into_preview();
        assert_eq!(preview.warnings.len(), 1);
        assert_snapshot!(format_parse_error(input, preview.warnings.into_iter().next().unwrap()), @r###"
         ⚠ The preview features: foobar, new_parsing are defined in the manifest but un-used in pixi
          ╭─[pixi.toml:1:13]
        1 │ preview = ["foobar", "pixi-build", "new_parsing"]
          ·             ───┬──                  ─────┬─────
          ·                │                         ╰── 'new_parsing' is unknown
          ·                ╰── 'foobar' is unknown
          ╰────
        "###);
    }
}
