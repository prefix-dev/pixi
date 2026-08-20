use std::{ops::Range, str::FromStr};

use itertools::Itertools;
use miette::LabeledSpan;
use toml_span::{DeserError, Spanned, Value, de_helpers::expected, value::ValueInner};

use crate::{KnownPreviewFlag, Preview, WithWarnings, error::GenericError};

#[derive(Debug, Clone, PartialEq)]
/// The preview flags of the project
pub enum TomlPreview {
    /// All preview flags are enabled
    AllEnabled(Spanned<bool>), // For `preview = true`
    /// Specific preview flags are enabled
    Flags(Vec<Spanned<KnownOrUnknownPreviewFlag>>), // For `preview = ["pixi-build"]`
}

impl Default for TomlPreview {
    fn default() -> Self {
        Self::Flags(Vec::new())
    }
}

impl TomlPreview {
    /// Returns the span of the definition of a certain flag.
    pub fn get_span(&self, flag: KnownPreviewFlag) -> Option<Range<usize>> {
        match self {
            TomlPreview::AllEnabled(enabled) => enabled.value.then(|| enabled.span.into()),
            TomlPreview::Flags(flags) => flags.iter().find_map(|f| {
                if f.value == KnownOrUnknownPreviewFlag::Known(flag) {
                    Some(f.span.into())
                } else {
                    None
                }
            }),
        }
    }

    /// Returns true if the given preview flag is enabled
    pub fn is_enabled(&self, flag: KnownPreviewFlag) -> bool {
        match self {
            Self::AllEnabled(_) => true,
            Self::Flags(flags) => flags
                .iter()
                .any(|f| f.value == KnownOrUnknownPreviewFlag::Known(flag)),
        }
    }
}

impl TomlPreview {
    pub fn into_preview(self) -> WithWarnings<Preview> {
        match self {
            TomlPreview::AllEnabled(all_enabled) => {
                WithWarnings::from(Preview::AllEnabled(all_enabled.value))
            }
            TomlPreview::Flags(flags) => {
                let mut known_flags = Vec::with_capacity(flags.len());
                let mut unknown_flags = Vec::new();
                for Spanned { value, span } in flags {
                    match value {
                        KnownOrUnknownPreviewFlag::Known(flag) => known_flags.push(flag),
                        KnownOrUnknownPreviewFlag::Unknown(name) => {
                            unknown_flags.push((name, span))
                        }
                    };
                }
                let preview = WithWarnings::from(Preview::Flags(known_flags));
                if unknown_flags.is_empty() {
                    preview
                } else {
                    let are = if unknown_flags.len() > 1 { "are" } else { "is" };
                    let s = if unknown_flags.len() > 1 { "s" } else { "" };
                    let warning = GenericError::new(format!(
                        "The preview flag{s}: {} {are} defined in the manifest but un-used in pixi",
                        unknown_flags.iter().map(|(name, _)| name).format(", ")
                    ))
                    .with_labels(unknown_flags.into_iter().map(
                        |(name, span)| {
                            LabeledSpan::new_with_span(
                                Some(format!("'{name}' is unknown")),
                                Range::<usize>::from(span),
                            )
                        },
                    ));
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
                let flags = arr
                    .into_iter()
                    .map(|mut value| toml_span::Deserialize::deserialize(&mut value))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(TomlPreview::Flags(flags))
            }
            other => Err(DeserError::from(expected(
                "bool or list of flags e.g `true` or `[\"pixi-build\"]`",
                other,
                value.span,
            ))),
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for KnownOrUnknownPreviewFlag {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let str = value.take_string("a preview flag name".into())?;
        Ok(KnownPreviewFlag::from_str(&str).map_or_else(
            |_| KnownOrUnknownPreviewFlag::Unknown(str.into_owned()),
            KnownOrUnknownPreviewFlag::Known,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnownOrUnknownPreviewFlag {
    Known(KnownPreviewFlag),
    Unknown(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toml::{FromTomlStr, preview::KnownOrUnknownPreviewFlag::Unknown};
    use assert_matches::assert_matches;
    use insta::assert_snapshot;
    use pixi_test_utils::format_parse_error;
    use toml_span::de_helpers::TableHelper;

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
    fn test_preview_with_unknown_flag() {
        let input = r#"preview = ["build"]"#;
        let top = TopLevel::from_toml_str(input).expect("should parse as `Flags` with known flag");
        match top.preview {
            TomlPreview::Flags(vec) => {
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
         × expected bool or list of flags e.g `true` or `["pixi-build"]`, found string
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
         × expected a preview flag name, found integer
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
         × expected bool or list of flags e.g `true` or `["pixi-build"]`, found integer
          ╭─[pixi.toml:1:11]
        1 │ preview = 123
          ·           ───
          ╰────
        "###
        );
    }

    #[test]
    fn test_flag_is_unknown() {
        let input = r#"preview = ["new_parsing"]"#;
        let top = TopLevel::from_toml_str(input).unwrap();
        match top.preview {
            TomlPreview::AllEnabled(_) => unreachable!("this arm should not be used"),
            TomlPreview::Flags(vec) => {
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
    fn test_unknown_flag_warning() {
        let input = r#"preview = ["foobar", "pixi-build", "new_parsing"]"#;
        let top = TopLevel::from_toml_str(input).unwrap();
        let preview = top.preview.into_preview();
        assert_eq!(preview.warnings.len(), 1);
        assert_snapshot!(format_parse_error(input, preview.warnings.into_iter().next().unwrap()), @r###"
         ⚠ The preview flags: foobar, new_parsing are defined in the manifest but un-used in pixi
          ╭─[pixi.toml:1:13]
        1 │ preview = ["foobar", "pixi-build", "new_parsing"]
          ·             ───┬──                  ─────┬─────
          ·                │                         ╰── 'new_parsing' is unknown
          ·                ╰── 'foobar' is unknown
          ╰────
        "###);
    }
}
