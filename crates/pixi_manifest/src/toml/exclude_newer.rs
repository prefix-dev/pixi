use std::str::FromStr;

use indexmap::IndexMap;
use itertools::Itertools;
use pixi_pypi_spec::{PypiPackageName, VersionOrStar};
use pixi_spec::{BinarySpec, DetailedSpec, ExcludeNewer, SpecError, TomlSpec};
use pixi_toml::TomlFromStr;
use rattler_conda_types::PackageName;
use toml_span::{
    DeserError, Deserialize, ErrorKind, Span, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

use crate::{ExcludeNewerConfig, PypiExcludeNewerConfig};

/// The TOML layout of `[workspace.exclude-newer]`.
///
/// Supports a bare cutoff or a table with a cutoff and exemptions:
///
/// ```toml
/// exclude-newer = "7d"
/// exclude-newer = { cutoff = "7d", exemptions = { polars = "==1.43.1" } }
/// ```
#[derive(Debug, Clone)]
pub struct TomlExcludeNewer(pub ExcludeNewerConfig);

impl TomlExcludeNewer {
    pub fn into_inner(self) -> ExcludeNewerConfig {
        self.0
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlExcludeNewer {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        // Unlike `pypi-exclude-newer`, there is no cutoff to fall back to, so
        // exemptions without a cutoff would do nothing.
        let (cutoff, exemptions) =
            parse_cutoff_and_exemptions(value, true, parse_conda_exemptions)?;
        Ok(Self(ExcludeNewerConfig { cutoff, exemptions }))
    }
}

/// Parses either a bare cutoff string or a table with a `cutoff` and
/// `exemptions`, the latter parsed by `parse_exemptions`.
fn parse_cutoff_and_exemptions<'de, E: Default>(
    value: &mut Value<'de>,
    cutoff_required: bool,
    parse_exemptions: impl FnOnce(&mut Value<'de>) -> Result<E, DeserError>,
) -> Result<(Option<ExcludeNewer>, E), DeserError> {
    match value.take() {
        ValueInner::String(cutoff) => {
            let cutoff = ExcludeNewer::from_str(&cutoff).map_err(|err| {
                DeserError::from(toml_span::Error {
                    kind: ErrorKind::Custom(err.into()),
                    span: value.span,
                    line_info: None,
                })
            })?;
            Ok((Some(cutoff), E::default()))
        }
        inner @ ValueInner::Table(_) => {
            let mut table = Value::with_span(inner, value.span);
            let mut th = TableHelper::new(&mut table)?;
            let cutoff = if cutoff_required {
                th.required::<TomlFromStr<ExcludeNewer>>("cutoff").ok()
            } else {
                th.optional::<TomlFromStr<ExcludeNewer>>("cutoff")
            }
            .map(TomlFromStr::into_inner);
            let exemptions = match th.take("exemptions") {
                Some((_, mut exemptions)) => {
                    parse_exemptions(&mut exemptions).unwrap_or_else(|err| {
                        th.errors.extend(err.errors);
                        E::default()
                    })
                }
                None => E::default(),
            };
            th.finalize(None)?;
            Ok((cutoff, exemptions))
        }
        other => Err(expected(
            "a cutoff string or a table with `cutoff` and `exemptions`",
            other,
            value.span,
        )
        .into()),
    }
}

/// Records the span of every exemption name that was parsed so far and
/// reports a [`ErrorKind::DuplicateKey`] when a name shows up twice. Package
/// names are normalized, so this also catches keys that differ only in case,
/// like `polars` and `Polars`, which would otherwise silently overwrite each
/// other.
fn check_duplicate_exemption<N: std::hash::Hash + Eq + Clone>(
    name_spans: &mut IndexMap<N, Span>,
    name: &N,
    key: &toml_span::value::Key<'_>,
    errors: &mut DeserError,
) -> bool {
    if let Some(first) = name_spans.get(name) {
        errors.errors.push(toml_span::Error {
            kind: ErrorKind::DuplicateKey {
                key: key.name.to_string(),
                first: *first,
            },
            span: key.span,
            line_info: None,
        });
        false
    } else {
        name_spans.insert(name.clone(), key.span);
        true
    }
}

/// Parses the `exemptions` table of `[workspace.exclude-newer]`. Every entry
/// is a package name mapped to a version string or a table with match spec
/// fields, like a dependency.
fn parse_conda_exemptions(
    value: &mut Value<'_>,
) -> Result<IndexMap<PackageName, BinarySpec>, DeserError> {
    let table = match value.take() {
        ValueInner::Table(table) => table,
        other => return Err(expected("a table of exemptions", other, value.span).into()),
    };

    let mut errors = DeserError { errors: vec![] };
    let mut exemptions = IndexMap::new();
    let mut name_spans = IndexMap::new();
    for (key, mut entry) in table.into_iter().sorted_by_key(|(k, _)| k.span.start) {
        let name = match PackageName::from_str(&key.name) {
            Ok(name) if check_duplicate_exemption(&mut name_spans, &name, &key, &mut errors) => {
                name
            }
            Ok(_) => continue,
            Err(err) => {
                errors.errors.push(toml_span::Error {
                    kind: ErrorKind::Custom(err.to_string().into()),
                    span: key.span,
                    line_info: None,
                });
                continue;
            }
        };

        let span = entry.span;
        let spec = match TomlSpec::deserialize_from_value(&mut entry) {
            Ok(spec) => spec,
            Err(err) => {
                errors.merge(err);
                continue;
            }
        };

        match binary_exemption_spec(spec) {
            Ok(spec) => {
                exemptions.insert(name, spec);
            }
            Err(err) => errors.errors.push(toml_span::Error {
                kind: ErrorKind::Custom(err.to_string().into()),
                span,
                line_info: None,
            }),
        }
    }

    if errors.errors.is_empty() {
        Ok(exemptions)
    } else {
        Err(errors)
    }
}

/// An exemption of `[workspace.exclude-newer]` that cannot select repodata
/// records.
#[derive(Debug, thiserror::Error)]
enum InvalidExemptionSpec {
    #[error("an exclude-newer exemption cannot have `extras`, they do not select packages")]
    Extras,

    #[error("an exclude-newer exemption cannot have `when`, it does not select packages")]
    Condition,

    #[error(
        "an exclude-newer exemption must be a version or a table with match spec fields such as `version`, `build` or `channel`"
    )]
    NotBinary,

    #[error(
        "an exclude-newer exemption table must specify at least one match spec field, use `\"*\"` to exempt every release"
    )]
    Empty,

    #[error(transparent)]
    Spec(#[from] SpecError),
}

/// Converts the TOML spec of an exemption into a binary spec, rejecting the
/// fields that cannot select repodata records.
fn binary_exemption_spec(spec: TomlSpec) -> Result<BinarySpec, InvalidExemptionSpec> {
    let spec = spec.into_binary_spec()?;
    match spec {
        BinarySpec::Version(_) => Ok(spec),
        BinarySpec::DetailedVersion(detailed) => {
            if detailed.extras.is_some() {
                return Err(InvalidExemptionSpec::Extras);
            }
            if detailed.condition.is_some() {
                return Err(InvalidExemptionSpec::Condition);
            }
            // An empty table would match every release, so a typo would
            // silently exempt the package entirely.
            if *detailed == DetailedSpec::default() {
                return Err(InvalidExemptionSpec::Empty);
            }
            // A bare version string parses into a detailed spec that only
            // carries a version. Keep it as the plain version it was written
            // as.
            let version_only = DetailedSpec {
                version: detailed.version.clone(),
                ..DetailedSpec::default()
            };
            match detailed.version.clone() {
                Some(version) if *detailed == version_only => Ok(BinarySpec::Version(version)),
                _ => Ok(BinarySpec::DetailedVersion(detailed)),
            }
        }
        BinarySpec::Url(_) | BinarySpec::Path(_) => Err(InvalidExemptionSpec::NotBinary),
    }
}

/// The TOML layout of `[workspace.pypi-exclude-newer]`.
///
/// Supports a bare cutoff or a table with a cutoff and exemptions:
///
/// ```toml
/// pypi-exclude-newer = "7d"
/// pypi-exclude-newer = { cutoff = "7d", exemptions = { torch = "*" } }
/// ```
#[derive(Debug, Clone)]
pub struct TomlPypiExcludeNewer(pub PypiExcludeNewerConfig);

impl TomlPypiExcludeNewer {
    pub fn into_inner(self) -> PypiExcludeNewerConfig {
        self.0
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlPypiExcludeNewer {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let (cutoff, exemptions) =
            parse_cutoff_and_exemptions(value, false, parse_pypi_exemptions)?;
        Ok(Self(PypiExcludeNewerConfig { cutoff, exemptions }))
    }
}

/// Parses the `exemptions` table of `[workspace.pypi-exclude-newer]`.
///
/// uv only supports lifting the cutoff for a whole package, so only the
/// wildcard `"*"` is accepted until version-specific exemptions are available
/// for PyPI packages.
fn parse_pypi_exemptions(
    value: &mut Value<'_>,
) -> Result<IndexMap<PypiPackageName, VersionOrStar>, DeserError> {
    let table = match value.take() {
        ValueInner::Table(table) => table,
        other => return Err(expected("a table of exemptions", other, value.span).into()),
    };

    let mut errors = DeserError { errors: vec![] };
    let mut exemptions = IndexMap::new();
    let mut name_spans = IndexMap::new();
    for (key, mut entry) in table.into_iter().sorted_by_key(|(k, _)| k.span.start) {
        let name = match PypiPackageName::from_str(&key.name) {
            Ok(name) if check_duplicate_exemption(&mut name_spans, &name, &key, &mut errors) => {
                name
            }
            Ok(_) => continue,
            Err(err) => {
                errors.errors.push(toml_span::Error {
                    kind: ErrorKind::Custom(err.to_string().into()),
                    span: key.span,
                    line_info: None,
                });
                continue;
            }
        };

        let span = entry.span;
        let version = match VersionOrStar::deserialize(&mut entry) {
            Ok(version) => version,
            Err(err) => {
                errors.merge(err);
                continue;
            }
        };

        match version {
            VersionOrStar::Star => {
                exemptions.insert(name, version);
            }
            VersionOrStar::Version(_) => errors.errors.push(toml_span::Error {
                kind: ErrorKind::Custom(
                    format!(
                        "PyPI exclude-newer exemptions can only exempt every release of a package for now, use `{} = \"*\"`",
                        name.as_source()
                    )
                    .into(),
                ),
                span,
                line_info: None,
            }),
        }
    }

    if errors.errors.is_empty() {
        Ok(exemptions)
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use pixi_test_utils::format_parse_error;

    use super::*;

    fn parse_conda(input: &str) -> Result<ExcludeNewerConfig, DeserError> {
        let mut value = toml_span::parse(input).unwrap();
        let mut th = TableHelper::new(&mut value).unwrap();
        let config = th.required::<TomlExcludeNewer>("exclude-newer")?;
        th.finalize(None)?;
        Ok(config.into_inner())
    }

    fn parse_pypi(input: &str) -> Result<PypiExcludeNewerConfig, DeserError> {
        let mut value = toml_span::parse(input).unwrap();
        let mut th = TableHelper::new(&mut value).unwrap();
        let config = th.required::<TomlPypiExcludeNewer>("pypi-exclude-newer")?;
        th.finalize(None)?;
        Ok(config.into_inner())
    }

    fn render_error(input: &str, err: DeserError) -> String {
        format_parse_error(input, crate::TomlError::from(err))
    }

    #[test]
    fn test_cutoff_string() {
        let config = parse_conda(r#"exclude-newer = "7d""#).unwrap();
        assert_eq!(config.cutoff.unwrap().to_string(), "7days");
        assert!(config.exemptions.is_empty());
    }

    #[test]
    fn test_table_with_exemptions() {
        let config = parse_conda(
            r#"
exclude-newer = { cutoff = "2025-01-01", exemptions = { polars = "1.43.1", py-rattler = "*", numpy = { version = "2.*", channel = "conda-forge", build = "py313*" } } }
"#,
        )
        .unwrap();
        assert_eq!(
            config.cutoff.unwrap().to_string(),
            "2025-01-02 00:00:00 UTC"
        );
        assert_snapshot!(
            serde_json::to_string_pretty(&config.exemptions).unwrap(),
            @r#"
        {
          "polars": "==1.43.1",
          "py-rattler": "*",
          "numpy": {
            "version": "2.*",
            "build": "py313*",
            "channel": "conda-forge"
          }
        }
        "#
        );
    }

    #[test]
    fn test_exemptions_without_cutoff() {
        let input = r#"exclude-newer = { exemptions = { polars = "*" } }"#;
        let err = parse_conda(input).unwrap_err();
        assert_snapshot!(render_error(input, err));
    }

    #[test]
    fn test_exemption_empty_table() {
        let input = r#"exclude-newer = { cutoff = "7d", exemptions = { polars = {} } }"#;
        let err = parse_conda(input).unwrap_err();
        assert_snapshot!(render_error(input, err));
    }

    #[test]
    fn test_exemption_duplicate_name() {
        let input = r#"
[exclude-newer]
cutoff = "7d"

[exclude-newer.exemptions]
polars = "1.0"
Polars = "2.0"
"#;
        let err = parse_conda(input).unwrap_err();
        assert_snapshot!(render_error(input, err));
    }

    #[test]
    fn test_pypi_exemption_duplicate_name() {
        let input = r#"
[pypi-exclude-newer]
cutoff = "7d"

[pypi-exclude-newer.exemptions]
scikit-learn = "*"
scikit_learn = "*"
"#;
        let err = parse_pypi(input).unwrap_err();
        assert_snapshot!(render_error(input, err));
    }

    #[test]
    fn test_invalid_cutoff() {
        let input = r#"exclude-newer = { cutoff = "date" }"#;
        let err = parse_conda(input).unwrap_err();
        assert_snapshot!(render_error(input, err));
    }

    #[test]
    fn test_unknown_field() {
        let input = r#"exclude-newer = { cutoff = "7d", exemption = { polars = "*" } }"#;
        let err = parse_conda(input).unwrap_err();
        assert_snapshot!(render_error(input, err));
    }

    #[test]
    fn test_exemption_with_path() {
        let input =
            r#"exclude-newer = { cutoff = "7d", exemptions = { polars = { path = "./polars" } } }"#;
        let err = parse_conda(input).unwrap_err();
        assert_snapshot!(render_error(input, err));
    }

    #[test]
    fn test_exemption_with_extras() {
        let input = r#"exclude-newer = { cutoff = "7d", exemptions = { polars = { version = "*", extras = ["foo"] } } }"#;
        let err = parse_conda(input).unwrap_err();
        assert_snapshot!(render_error(input, err));
    }

    #[test]
    fn test_pypi_cutoff_string() {
        let config = parse_pypi(r#"pypi-exclude-newer = "7d""#).unwrap();
        assert_eq!(config.cutoff.unwrap().to_string(), "7days");
        assert!(config.exemptions.is_empty());
    }

    #[test]
    fn test_pypi_table_with_exemptions() {
        let config =
            parse_pypi(r#"pypi-exclude-newer = { cutoff = "7d", exemptions = { Torch = "*" } }"#)
                .unwrap();
        assert_eq!(config.cutoff.unwrap().to_string(), "7days");
        assert_eq!(
            config
                .exemptions
                .keys()
                .map(|name| name.as_normalized().to_string())
                .collect::<Vec<_>>(),
            vec!["torch"]
        );
    }

    #[test]
    fn test_pypi_exemption_with_version_is_rejected() {
        let input = r#"pypi-exclude-newer = { exemptions = { torch = "==2.10.0" } }"#;
        let err = parse_pypi(input).unwrap_err();
        assert_snapshot!(render_error(input, err));
    }
}
