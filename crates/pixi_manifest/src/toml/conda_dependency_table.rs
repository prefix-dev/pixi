use std::path::PathBuf;

use itertools::Itertools;
use pixi_spec::PixiSpec;
use rattler_conda_types::PackageName;
use std::str::FromStr;
use toml_span::{DeserError, Value, de_helpers::expected, value::ValueInner};

use crate::utils::{PixiSpanned, package_map::UniquePackageMap};

/// Reserved key in `[dependencies]` for pip-style requirements files (`pypi-txt` import format).
pub const PYPI_TXT_DEPENDENCY_KEY: &str = "pypi-txt";

/// Conda packages plus optional `pypi-txt` requirements file includes.
#[derive(Debug, Default, Clone)]
pub struct CondaDependencyTable {
    pub packages: UniquePackageMap,
    pub pypi_txt_paths: Vec<PathBuf>,
}

fn parse_pypi_txt_paths<'de>(value: &mut Value<'de>) -> Result<Vec<PathBuf>, DeserError> {
    let span = value.span;
    match value.take() {
        ValueInner::String(s) => Ok(vec![PathBuf::from(s.into_owned())]),
        ValueInner::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for mut item in arr {
                match item.take() {
                    ValueInner::String(s) => out.push(PathBuf::from(s.into_owned())),
                    inner => {
                        return Err(
                            expected("a string (requirements file path)", inner, item.span)
                                .into(),
                        );
                    }
                }
            }
            Ok(out)
        }
        inner => Err(
            expected(
                "a string or array of strings (requirements file paths)",
                inner,
                span,
            )
            .into(),
        ),
    }
}

impl<'de> toml_span::Deserialize<'de> for CondaDependencyTable {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let table = match value.take() {
            ValueInner::Table(table) => table,
            inner => return Err(expected("a table", inner, value.span).into()),
        };

        let mut errors = DeserError { errors: vec![] };
        let mut packages = UniquePackageMap::default();
        let mut pypi_txt_paths = Vec::new();

        for (key, mut value) in table.into_iter().sorted_by_key(|(k, _)| k.span.start) {
            if key.name.eq_ignore_ascii_case(PYPI_TXT_DEPENDENCY_KEY) {
                match parse_pypi_txt_paths(&mut value) {
                    Ok(mut paths) => pypi_txt_paths.append(&mut paths),
                    Err(e) => errors.merge(e),
                }
                continue;
            }

            let name = match PackageName::from_str(&key.name) {
                Ok(name) => {
                    if let Some(first) = packages.name_spans.get(&name) {
                        errors.errors.push(toml_span::Error {
                            kind: toml_span::ErrorKind::DuplicateKey {
                                key: key.name.into_owned(),
                                first: toml_span::Span {
                                    start: first.start,
                                    end: first.end,
                                },
                            },
                            span: key.span,
                            line_info: None,
                        });
                        None
                    } else {
                        Some(name)
                    }
                }
                Err(e) => {
                    errors.errors.push(toml_span::Error {
                        kind: toml_span::ErrorKind::Custom(e.to_string().into()),
                        span: key.span,
                        line_info: None,
                    });
                    None
                }
            };

            let spec: Option<PixiSpec> = match toml_span::Deserialize::deserialize(&mut value) {
                Ok(spec) => Some(spec),
                Err(e) => {
                    errors.merge(e);
                    None
                }
            };

            if let (Some(name), Some(spec)) = (name, spec) {
                packages.specs.insert(name.clone(), spec);
                packages
                    .name_spans
                    .insert(name.clone(), key.span.start..key.span.end);
                packages
                    .value_spans
                    .insert(name, value.span.start..value.span.end);
            }
        }

        if errors.errors.is_empty() {
            Ok(Self {
                packages,
                pypi_txt_paths,
            })
        } else {
            Err(errors)
        }
    }
}

impl CondaDependencyTable {
    pub fn into_spanned_unique_map(
        self,
        outer_span: Option<std::ops::Range<usize>>,
    ) -> (Option<PixiSpanned<UniquePackageMap>>, Vec<PathBuf>) {
        let pypi_txt_paths = self.pypi_txt_paths;
        if self.packages.specs.is_empty() {
            return (None, pypi_txt_paths);
        }
        (
            Some(PixiSpanned {
                value: self.packages,
                span: outer_span,
            }),
            pypi_txt_paths,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_conda_types::PackageName;

    use super::*;
    use crate::toml::FromTomlStr;

    #[test]
    fn deserializes_pypi_txt_paths() {
        let src = r#"
requests = ">=2"
pypi-txt = ["a.txt", "b.txt"]
"#;
        let parsed = CondaDependencyTable::from_toml_str(src).unwrap();
        assert_eq!(
            parsed.pypi_txt_paths,
            vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")]
        );
        assert!(parsed
            .packages
            .specs
            .contains_key(&PackageName::from_str("requests").unwrap()));
    }
}
