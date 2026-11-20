use std::{collections::HashMap, str::FromStr};

use indexmap::IndexMap;
use itertools::Itertools;
use pixi_pypi_spec::{PypiPackageName, VersionOrStar};
use pixi_spec::{BinarySpec, DetailedSpec, ExcludeNewer, SpecConversionError, SpecError, TomlSpec};
use pixi_toml::TomlFromStr;
use rattler_conda_types::{ChannelConfig, PackageName};
use rattler_config::config::exclude_newer::{
    InvalidExemptionSpecError, validate_exemption_spec as validate_rattler_exemption_spec,
};
use toml_span::{
    DeserError, Deserialize, ErrorKind, Span, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

use crate::{ExcludeNewerConfig, ExcludeNewerExemption, PypiExcludeNewerConfig};

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
    name_spans: &mut HashMap<N, Span>,
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
/// fields, like a dependency, and an optional absolute `cutoff`.
fn parse_conda_exemptions(
    value: &mut Value<'_>,
) -> Result<IndexMap<PackageName, ExcludeNewerExemption>, DeserError> {
    let table = match value.take() {
        ValueInner::Table(table) => table,
        other => return Err(expected("a table of exemptions", other, value.span).into()),
    };

    let mut errors = DeserError { errors: vec![] };
    let mut exemptions = IndexMap::new();
    let mut name_spans = HashMap::new();
    for (key, mut entry) in table.into_iter().sorted_by_key(|(k, _)| k.span.start) {
        let name = match PackageName::from_str(&key.name) {
            Ok(name) => name,
            Err(err) => {
                errors.errors.push(toml_span::Error {
                    kind: ErrorKind::Custom(err.to_string().into()),
                    span: key.span,
                    line_info: None,
                });
                continue;
            }
        };
        if !check_duplicate_exemption(&mut name_spans, &name, &key, &mut errors) {
            continue;
        }

        let span = entry.span;
        let cutoff = match take_exemption_cutoff(&mut entry) {
            Ok(cutoff) => cutoff,
            Err(err) => {
                errors.errors.push(err);
                continue;
            }
        };
        let spec = match TomlSpec::deserialize_from_value(&mut entry) {
            Ok(spec) => spec,
            Err(err) => {
                errors.merge(err);
                continue;
            }
        };

        match binary_exemption_spec(&name, spec) {
            Ok(spec) => {
                exemptions.insert(name, ExcludeNewerExemption { spec, cutoff });
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

/// Removes the `cutoff` of an exemption table from `entry` and parses it,
/// leaving the match spec fields behind.
fn take_exemption_cutoff(entry: &mut Value<'_>) -> Result<Option<ExcludeNewer>, toml_span::Error> {
    let mut table = match entry.take() {
        ValueInner::Table(table) => table,
        other => {
            entry.set(other);
            return Ok(None);
        }
    };
    let value = table.remove("cutoff");
    entry.set(ValueInner::Table(table));
    let Some(mut value) = value else {
        return Ok(None);
    };
    let span = value.span;
    let cutoff = value.take_string(None)?;
    ExcludeNewer::from_str(&cutoff)
        .map(Some)
        .map_err(|message| toml_span::Error {
            kind: ErrorKind::Custom(message.into()),
            span,
            line_info: None,
        })
}

/// An exemption of `[workspace.exclude-newer]` that cannot select repodata
/// records.
#[derive(Debug, thiserror::Error)]
pub enum InvalidExemptionSpec {
    #[error(
        "an exclude-newer exemption must be a version, a table with match spec fields such as `version`, `build` or `channel`, or a package archive"
    )]
    NotBinary,

    /// `rattler_config` only exempts a package archive by its `url`.
    #[error("an exclude-newer exemption must use a `url` instead of a `path`")]
    Path,

    /// The exemption breaks one of the rules of `rattler_config`, e.g. it is
    /// an empty table or has `extras`, which don't select packages.
    #[error(transparent)]
    Rattler(#[from] InvalidExemptionSpecError),

    #[error(transparent)]
    Spec(#[from] SpecError),

    #[error(transparent)]
    Conversion(#[from] SpecConversionError),
}

/// Converts the TOML spec of an exemption of `name` into a binary spec,
/// rejecting the fields that cannot select repodata records.
fn binary_exemption_spec(
    name: &PackageName,
    spec: TomlSpec,
) -> Result<BinarySpec, InvalidExemptionSpec> {
    validate_exemption_spec(name, spec.into_binary_spec()?)
}

/// Checks that a binary spec can be used as an exemption of `name`, rejecting
/// the fields that cannot select repodata records. The rules are those of
/// `rattler_config`, this only applies them to pixi's spec, so the manifest
/// reports them with a span.
pub fn validate_exemption_spec(
    name: &PackageName,
    spec: BinarySpec,
) -> Result<BinarySpec, InvalidExemptionSpec> {
    match spec {
        BinarySpec::Version(_) => Ok(spec),
        BinarySpec::DetailedVersion(detailed) => {
            // The channel doesn't affect the rules, so it is resolved with the
            // default configuration.
            let channel_config =
                ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap_or_default());
            let nameless = (*detailed)
                .clone()
                .try_into_nameless_match_spec(&channel_config)?;
            validate_rattler_exemption_spec(name, &nameless)?;
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
        // An exemption of a package archive, like the `url` of an exemption
        // in `rattler_config`.
        BinarySpec::Url(_) => Ok(spec),
        BinarySpec::Path(_) => Err(InvalidExemptionSpec::Path),
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
    let mut name_spans = HashMap::new();
    for (key, mut entry) in table.into_iter().sorted_by_key(|(k, _)| k.span.start) {
        let name = match PypiPackageName::from_str(&key.name) {
            Ok(name) => name,
            Err(err) => {
                errors.errors.push(toml_span::Error {
                    kind: ErrorKind::Custom(err.to_string().into()),
                    span: key.span,
                    line_info: None,
                });
                continue;
            }
        };
        if !check_duplicate_exemption(&mut name_spans, &name, &key, &mut errors) {
            continue;
        }

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
    fn test_url_exemption() {
        let config = parse_conda(
            r#"
exclude-newer = { cutoff = "7d", exemptions = { ruff = { url = "https://conda.anaconda.org/conda-forge/noarch/ruff-0.1-0.conda", cutoff = "2026-10-12" } } }
"#,
        )
        .unwrap();
        assert_snapshot!(
            serde_json::to_string_pretty(&config.exemptions).unwrap(),
            @r#"
        {
          "ruff": {
            "url": "https://conda.anaconda.org/conda-forge/noarch/ruff-0.1-0.conda",
            "cutoff": "2026-10-13T00:00:00Z"
          }
        }
        "#
        );
    }

    #[test]
    fn test_exemption_with_cutoff() {
        let config = parse_conda(
            r#"
exclude-newer = { cutoff = "7d", exemptions = { openssl = { version = "==3.2.1", cutoff = "2026-10-12T08:00:00Z" }, polars = { version = "*", channel = "conda-forge", cutoff = "2026-10-12" } } }
"#,
        )
        .unwrap();
        assert_snapshot!(
            serde_json::to_string_pretty(&config.exemptions).unwrap(),
            @r#"
        {
          "openssl": {
            "version": "==3.2.1",
            "cutoff": "2026-10-12T08:00:00Z"
          },
          "polars": {
            "version": "*",
            "channel": "conda-forge",
            "cutoff": "2026-10-13T00:00:00Z"
          }
        }
        "#
        );
    }

    #[test]
    fn test_exemption_with_relative_cutoff() {
        let config = parse_conda(
            r#"exclude-newer = { cutoff = "7d", exemptions = { openssl = { version = "==3.2.1", cutoff = "1d" } } }"#,
        )
        .unwrap();
        assert_eq!(
            config.exemptions[&PackageName::new_unchecked("openssl")].cutoff,
            Some(ExcludeNewer::Duration(std::time::Duration::from_secs(
                24 * 60 * 60
            )))
        );
    }

    #[test]
    fn test_exemption_with_invalid_cutoff() {
        let input = r#"exclude-newer = { cutoff = "7d", exemptions = { openssl = { version = "==3.2.1", cutoff = "now" } } }"#;
        let err = parse_conda(input).unwrap_err();
        assert_snapshot!(render_error(input, err));
    }

    #[test]
    fn test_exemption_with_only_cutoff() {
        let input = r#"exclude-newer = { cutoff = "7d", exemptions = { openssl = { cutoff = "2026-10-12" } } }"#;
        let err = parse_conda(input).unwrap_err();
        assert_snapshot!(render_error(input, err));
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
        let input = r#"exclude-newer = { cutoff = "7d", exemptions = { polars = { path = "./polars-1.0-0.conda" } } }"#;
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
