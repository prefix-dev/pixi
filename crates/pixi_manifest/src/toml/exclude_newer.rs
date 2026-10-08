use std::str::FromStr;

use indexmap::IndexMap;
use pixi_pypi_spec::{PypiPackageName, VersionOrStar};
use pixi_spec::{BinarySpec, DetailedSpec, ExcludeNewer, TomlSpec};
use pixi_toml::TomlFromStr;
use rattler_conda_types::PackageName;
use toml_span::{
    DeserError, Deserialize, ErrorKind, Value,
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
/// exclude-newer = { cutoff = "7d", exemptions = { polars = "1.43.1" } }
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
        match value.take() {
            ValueInner::String(cutoff) => {
                let cutoff = ExcludeNewer::from_str(&cutoff).map_err(|err| {
                    DeserError::from(toml_span::Error {
                        kind: ErrorKind::Custom(err.into()),
                        span: value.span,
                        line_info: None,
                    })
                })?;
                Ok(Self(ExcludeNewerConfig::from_cutoff(cutoff)))
            }
            inner @ ValueInner::Table(_) => {
                let mut table = Value::with_span(inner, value.span);
                let mut th = TableHelper::new(&mut table)?;
                let cutoff = th
                    .optional::<TomlFromStr<ExcludeNewer>>("cutoff")
                    .map(TomlFromStr::into_inner);
                let exemptions = match th.take("exemptions") {
                    Some((_, mut exemptions)) => match parse_conda_exemptions(&mut exemptions) {
                        Ok(exemptions) => exemptions,
                        Err(err) => {
                            th.errors.extend(err.errors);
                            IndexMap::new()
                        }
                    },
                    None => IndexMap::new(),
                };
                th.finalize(None)?;
                Ok(Self(ExcludeNewerConfig { cutoff, exemptions }))
            }
            other => Err(expected(
                "a cutoff string or a table with `cutoff` and `exemptions`",
                other,
                value.span,
            )
            .into()),
        }
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
    for (key, mut entry) in table {
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
            Err(message) => errors.errors.push(toml_span::Error {
                kind: ErrorKind::Custom(message.into()),
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

/// Converts the TOML spec of an exemption into a binary spec, rejecting the
/// fields that cannot select repodata records.
fn binary_exemption_spec(spec: TomlSpec) -> Result<BinarySpec, String> {
    let spec = spec.into_binary_spec().map_err(|err| err.to_string())?;
    match spec {
        BinarySpec::Version(_) => Ok(spec),
        BinarySpec::DetailedVersion(detailed) => {
            if detailed.extras.is_some() {
                return Err(
                    "an exclude-newer exemption cannot have `extras`, they do not select packages"
                        .to_string(),
                );
            }
            if detailed.condition.is_some() {
                return Err(
                    "an exclude-newer exemption cannot have `when`, it does not select packages"
                        .to_string(),
                );
            }
            // A bare version string parses into a detailed spec that only
            // carries a version. Keep it as the plain version it was written
            // as.
            match *detailed {
                DetailedSpec {
                    version: Some(version),
                    build: None,
                    build_number: None,
                    file_name: None,
                    extras: None,
                    flags: None,
                    channel: None,
                    subdir: None,
                    license: None,
                    license_family: None,
                    condition: None,
                    track_features: None,
                    md5: None,
                    sha256: None,
                } => Ok(BinarySpec::Version(version)),
                detailed => Ok(BinarySpec::DetailedVersion(Box::new(detailed))),
            }
        }
        BinarySpec::Url(_) | BinarySpec::Path(_) => Err(
            "an exclude-newer exemption must be a version or a table with match spec fields such as `version`, `build` or `channel`"
                .to_string(),
        ),
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
        match value.take() {
            ValueInner::String(cutoff) => {
                let cutoff = ExcludeNewer::from_str(&cutoff).map_err(|err| {
                    DeserError::from(toml_span::Error {
                        kind: ErrorKind::Custom(err.into()),
                        span: value.span,
                        line_info: None,
                    })
                })?;
                Ok(Self(PypiExcludeNewerConfig {
                    cutoff: Some(cutoff),
                    exemptions: IndexMap::new(),
                }))
            }
            inner @ ValueInner::Table(_) => {
                let mut table = Value::with_span(inner, value.span);
                let mut th = TableHelper::new(&mut table)?;
                let cutoff = th
                    .optional::<TomlFromStr<ExcludeNewer>>("cutoff")
                    .map(TomlFromStr::into_inner);
                let exemptions = match th.take("exemptions") {
                    Some((_, mut exemptions)) => match parse_pypi_exemptions(&mut exemptions) {
                        Ok(exemptions) => exemptions,
                        Err(err) => {
                            th.errors.extend(err.errors);
                            IndexMap::new()
                        }
                    },
                    None => IndexMap::new(),
                };
                th.finalize(None)?;
                Ok(Self(PypiExcludeNewerConfig { cutoff, exemptions }))
            }
            other => Err(expected(
                "a cutoff string or a table with `cutoff` and `exemptions`",
                other,
                value.span,
            )
            .into()),
        }
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
    for (key, mut entry) in table {
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
          "numpy": {
            "version": "2.*",
            "build": "py313*",
            "channel": "conda-forge"
          },
          "polars": "==1.43.1",
          "py-rattler": "*"
        }
        "#
        );
    }

    #[test]
    fn test_exemptions_without_cutoff() {
        let config = parse_conda(r#"exclude-newer = { exemptions = { polars = "*" } }"#).unwrap();
        assert!(config.cutoff.is_none());
        assert_eq!(config.exemptions.len(), 1);
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
        let input = r#"exclude-newer = { exemptions = { polars = { path = "./polars" } } }"#;
        let err = parse_conda(input).unwrap_err();
        assert_snapshot!(render_error(input, err));
    }

    #[test]
    fn test_exemption_with_extras() {
        let input =
            r#"exclude-newer = { exemptions = { polars = { version = "*", extras = ["foo"] } } }"#;
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
