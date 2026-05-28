use std::{ops::Range, str::FromStr};

use indexmap::IndexMap;
use itertools::Itertools;
use pixi_spec::{PixiSpec, TomlSpec};
use rattler_conda_types::PackageName;
use toml_span::{
    DeserError, Span, Value,
    de_helpers::expected,
    value::{Table, ValueInner},
};

use crate::{TomlError, error::GenericError, utils::package_map::UniquePackageMap};

/// Entry in a `[package.*-dependencies]` table that may inherit from
/// `[workspace.dependencies]`.
#[derive(Debug)]
pub enum InheritableSpec {
    Direct(PixiSpec),
    /// Inherited from the workspace pool. `overrides.version` is always `None`.
    Inherited {
        marker_span: Range<usize>,
        overrides: Box<TomlSpec>,
    },
    /// `{ workspace = false }`; rejected at resolve time.
    NotWorkspace {
        marker_span: Range<usize>,
    },
}

/// Dependency map that may declare workspace inheritance. Resolve against the
/// pool to obtain a regular [`UniquePackageMap`].
#[derive(Default, Debug)]
pub struct InheritablePackageMap {
    pub specs: IndexMap<PackageName, InheritableSpec>,
    pub name_spans: IndexMap<PackageName, Range<usize>>,
    pub value_spans: IndexMap<PackageName, Range<usize>>,
}

impl InheritablePackageMap {
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Resolve every entry against the pool. Source specs require the
    /// `pixi-build` preview, matching the non-inheritable tables.
    pub fn resolve(
        self,
        workspace_deps: &IndexMap<PackageName, TomlSpec>,
        is_pixi_build_enabled: bool,
    ) -> Result<UniquePackageMap, TomlError> {
        let mut out = UniquePackageMap::default();
        for (name, spec) in self.specs {
            let name_span = self.name_spans.get(&name).cloned();
            let value_span = self.value_spans.get(&name).cloned();
            let resolved = match spec {
                InheritableSpec::Direct(spec) => spec,
                InheritableSpec::NotWorkspace { marker_span } => {
                    return Err(GenericError::new("`workspace` cannot be false")
                        .with_help("Remove the `workspace = false` entry; inheritance from the workspace is opt-in.")
                        .with_span(marker_span)
                        .into());
                }
                InheritableSpec::Inherited {
                    marker_span,
                    overrides,
                } => {
                    let base = lookup_workspace_base(workspace_deps, &name, &marker_span)?;
                    finalize_inherited(base, *overrides, &marker_span)?
                }
            };
            if let Some(span) = name_span.clone() {
                out.name_spans.insert(name.clone(), span);
            }
            if let Some(span) = value_span.clone() {
                out.value_spans.insert(name.clone(), span);
            }
            out.specs.insert(name, resolved);
        }
        if !is_pixi_build_enabled
            && let Some((package_name, _)) = out.specs.iter().find(|(_, spec)| spec.is_source())
        {
            return Err(TomlError::Generic(
                GenericError::new(
                    "conda source dependencies are not allowed without enabling the 'pixi-build' preview feature",
                )
                .with_opt_span(out.value_spans.get(package_name).cloned())
                .with_span_label("source dependency specified here")
                .with_help(
                    "Add `preview = [\"pixi-build\"]` to the `workspace` or `project` table of your manifest",
                ),
            ));
        }
        Ok(out)
    }
}

/// Resolve `{ name, workspace = true, ... }` build-backend entries.
pub fn resolve_inherited_backend_spec(
    name: &PackageName,
    workspace_deps: &IndexMap<PackageName, TomlSpec>,
    overrides: TomlSpec,
    marker_span: Range<usize>,
) -> Result<PixiSpec, TomlError> {
    let base = lookup_workspace_base(workspace_deps, name, &marker_span)?;
    finalize_inherited(base, overrides, &marker_span)
}

fn lookup_workspace_base(
    workspace_deps: &IndexMap<PackageName, TomlSpec>,
    name: &PackageName,
    marker_span: &Range<usize>,
) -> Result<TomlSpec, TomlError> {
    workspace_deps.get(name).cloned().ok_or_else(|| {
        TomlError::from(
            GenericError::new(format!(
                "the workspace does not define `{}` in `[workspace.dependencies]`",
                name.as_source()
            ))
            .with_help(
                "Add the package to `[workspace.dependencies]` in the workspace `pixi.toml`.",
            )
            .with_span(marker_span.clone()),
        )
    })
}

/// Workspace owns the version; member may layer non-version attributes.
/// Source-vs-binary conflicts are caught by `TomlSpec::into_spec`.
fn finalize_inherited(
    base: TomlSpec,
    overrides: TomlSpec,
    marker_span: &Range<usize>,
) -> Result<PixiSpec, TomlError> {
    base.layer_overrides(overrides).into_spec().map_err(|e| {
        GenericError::new(format!(
            "cannot apply member overrides to inherited workspace dependency: {e}"
        ))
        .with_span(marker_span.clone())
        .into()
    })
}

impl<'de> toml_span::Deserialize<'de> for InheritablePackageMap {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let table = match value.take() {
            ValueInner::Table(table) => table,
            inner => return Err(expected("a table", inner, value.span).into()),
        };

        let mut errors = DeserError { errors: vec![] };
        let mut result = Self::default();
        for (key, mut entry_value) in table.into_iter().sorted_by_key(|(k, _)| k.span.start) {
            let name = match PackageName::from_str(&key.name) {
                Ok(name) => {
                    if let Some(first) = result.name_spans.get(&name) {
                        errors.errors.push(toml_span::Error {
                            kind: toml_span::ErrorKind::DuplicateKey {
                                key: key.name.into_owned(),
                                first: Span {
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

            let entry_span = entry_value.span;
            let spec = match parse_inheritable_entry(&mut entry_value) {
                Ok(spec) => Some(spec),
                Err(e) => {
                    errors.merge(e);
                    None
                }
            };

            if let (Some(name), Some(spec)) = (name, spec) {
                result.specs.insert(name.clone(), spec);
                result
                    .name_spans
                    .insert(name.clone(), key.span.start..key.span.end);
                result
                    .value_spans
                    .insert(name, entry_span.start..entry_span.end);
            }
        }

        if errors.errors.is_empty() {
            Ok(result)
        } else {
            Err(errors)
        }
    }
}

fn parse_inheritable_entry(value: &mut Value<'_>) -> Result<InheritableSpec, DeserError> {
    let outer_span = value.span;
    match value.take() {
        ValueInner::String(s) => {
            let mut tmp = Value::with_span(ValueInner::String(s), outer_span);
            let spec = <PixiSpec as toml_span::Deserialize>::deserialize(&mut tmp)?;
            Ok(InheritableSpec::Direct(spec))
        }
        ValueInner::Table(mut table) => {
            if let Some(ws_val) = table.remove("workspace") {
                parse_workspace_marker(ws_val, table, outer_span)
            } else {
                let mut tmp = Value::with_span(ValueInner::Table(table), outer_span);
                let spec = <PixiSpec as toml_span::Deserialize>::deserialize(&mut tmp)?;
                Ok(InheritableSpec::Direct(spec))
            }
        }
        other => Err(expected("a string or a table", other, outer_span).into()),
    }
}

fn parse_workspace_marker<'de>(
    mut ws_val: Value<'de>,
    remaining: Table<'de>,
    outer_span: Span,
) -> Result<InheritableSpec, DeserError> {
    let marker_span = ws_val.span;
    let ws_bool = match ws_val.take() {
        ValueInner::Boolean(b) => b,
        other => return Err(expected("a boolean", other, marker_span).into()),
    };

    if !ws_bool {
        if !remaining.is_empty() {
            return Err(toml_span::Error {
                kind: toml_span::ErrorKind::Custom(
                    "`workspace = false` cannot be combined with other fields".into(),
                ),
                span: outer_span,
                line_info: None,
            }
            .into());
        }
        return Ok(InheritableSpec::NotWorkspace {
            marker_span: marker_span.start..marker_span.end,
        });
    }

    // workspace = true: remaining keys are TomlSpec overrides; `version` is rejected.
    let version_key_span = remaining
        .iter()
        .find(|(k, _)| k.name == "version")
        .map(|(k, _)| k.span);

    let mut overrides = if remaining.is_empty() {
        TomlSpec::empty()
    } else {
        let mut tmp = Value::with_span(ValueInner::Table(remaining), outer_span);
        <TomlSpec as toml_span::Deserialize>::deserialize(&mut tmp)?
    };

    if overrides.version.is_some() {
        return Err(toml_span::Error {
            kind: toml_span::ErrorKind::Custom(
                "`version` is mutual exclusive with `workspace`".into(),
            ),
            span: version_key_span.unwrap_or(marker_span),
            line_info: None,
        }
        .into());
    }
    overrides.version = None;

    Ok(InheritableSpec::Inherited {
        marker_span: marker_span.start..marker_span.end,
        overrides: Box::new(overrides),
    })
}

#[cfg(test)]
mod test {
    use indexmap::IndexMap;
    use pixi_spec::{PixiSpec, TomlSpec};
    use rattler_conda_types::PackageName;
    use std::str::FromStr;

    use super::*;
    use crate::toml::FromTomlStr;

    /// Build a pool from `(name, spec_toml)` pairs where `spec_toml` is the
    /// raw RHS of a TOML entry (quoted string or inline table).
    fn pool(entries: &[(&str, &str)]) -> IndexMap<PackageName, TomlSpec> {
        use crate::toml::TomlWorkspace;
        let mut doc =
            String::from("name = \"ws\"\nchannels = []\nplatforms = []\n[dependencies]\n");
        for (name, spec_toml) in entries {
            doc.push_str(&format!("{name} = {spec_toml}\n"));
        }
        TomlWorkspace::from_toml_str(&doc)
            .expect("parse workspace")
            .dependencies
            .expect("dependencies table")
            .value
            .specs
    }

    #[test]
    fn parses_direct_string_spec() {
        let input = r#"numpy = "1.*""#;
        let parsed = InheritablePackageMap::from_toml_str(input).unwrap();
        let entry = parsed
            .specs
            .get(&PackageName::from_str("numpy").unwrap())
            .unwrap();
        assert!(matches!(entry, InheritableSpec::Direct(_)));
    }

    #[test]
    fn parses_workspace_marker_only() {
        let input = r#"numpy = { workspace = true }"#;
        let parsed = InheritablePackageMap::from_toml_str(input).unwrap();
        let entry = parsed
            .specs
            .get(&PackageName::from_str("numpy").unwrap())
            .unwrap();
        assert!(matches!(entry, InheritableSpec::Inherited { .. }));
    }

    #[test]
    fn parses_workspace_marker_dotted_key_form() {
        // The dotted-key form is equivalent to `numpy = { workspace = true }`.
        let input = "numpy.workspace = true";
        let parsed = InheritablePackageMap::from_toml_str(input).unwrap();
        let entry = parsed
            .specs
            .get(&PackageName::from_str("numpy").unwrap())
            .unwrap();
        assert!(matches!(entry, InheritableSpec::Inherited { .. }));
    }

    #[test]
    fn parses_workspace_marker_with_overrides() {
        let input = r#"numpy = { workspace = true, channel = "conda-forge" }"#;
        let parsed = InheritablePackageMap::from_toml_str(input).unwrap();
        let entry = parsed
            .specs
            .get(&PackageName::from_str("numpy").unwrap())
            .unwrap();
        match entry {
            InheritableSpec::Inherited { overrides, .. } => {
                assert!(overrides.channel.is_some());
                assert!(overrides.version.is_none());
            }
            _ => panic!("expected inherited"),
        }
    }

    #[test]
    fn rejects_version_override_on_inherited_entry() {
        let input = r#"numpy = { workspace = true, version = "1.0" }"#;
        let err = InheritablePackageMap::from_toml_str(input).expect_err("must reject");
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("version") && msg.contains("workspace"),
            "unexpected error: {msg}",
        );
    }

    #[test]
    fn parses_workspace_false_as_not_workspace_variant() {
        let input = r#"numpy = { workspace = false }"#;
        let parsed = InheritablePackageMap::from_toml_str(input).unwrap();
        let entry = parsed
            .specs
            .get(&PackageName::from_str("numpy").unwrap())
            .unwrap();
        assert!(matches!(entry, InheritableSpec::NotWorkspace { .. }));
    }

    #[test]
    fn resolves_workspace_marker_to_pool_spec() {
        let input = r#"numpy = { workspace = true }"#;
        let map = InheritablePackageMap::from_toml_str(input).unwrap();
        let pool = pool(&[("numpy", "\"1.*\"")]);
        let resolved = map.resolve(&pool, true).unwrap();
        let spec = resolved
            .specs
            .get(&PackageName::from_str("numpy").unwrap())
            .unwrap();
        assert_eq!(spec.as_version_spec().unwrap().to_string(), "1.*",);
    }

    #[test]
    fn resolve_missing_workspace_entry_errors() {
        let input = r#"numpy = { workspace = true }"#;
        let map = InheritablePackageMap::from_toml_str(input).unwrap();
        let pool = pool(&[]);
        let err = map.resolve(&pool, true).expect_err("must error");
        assert!(format!("{err:?}").contains("workspace.dependencies"));
    }

    #[test]
    fn resolve_workspace_false_errors() {
        let input = r#"numpy = { workspace = false }"#;
        let map = InheritablePackageMap::from_toml_str(input).unwrap();
        let pool = pool(&[]);
        let err = map.resolve(&pool, true).expect_err("must error");
        assert!(format!("{err:?}").contains("workspace") && format!("{err:?}").contains("false"));
    }

    #[test]
    fn override_layers_channel_onto_version_base() {
        // Workspace defines just a version; member adds a channel override.
        // The merged spec must carry both.
        let input = r#"numpy = { workspace = true, channel = "conda-forge" }"#;
        let map = InheritablePackageMap::from_toml_str(input).unwrap();
        let pool = pool(&[("numpy", "\"1.*\"")]);
        let resolved = map.resolve(&pool, true).unwrap();
        let spec = resolved
            .specs
            .get(&PackageName::from_str("numpy").unwrap())
            .unwrap();
        match spec {
            PixiSpec::Detailed(detailed) => {
                assert_eq!(detailed.version.as_ref().unwrap().to_string(), "1.*");
                assert!(detailed.channel.is_some());
            }
            other => panic!("expected DetailedVersion, got {other:?}"),
        }
    }

    #[test]
    fn override_layers_onto_detailed_workspace_base() {
        // Workspace base is already detailed; member layers build matcher.
        let input = r#"boltons = { workspace = true, build = "py*" }"#;
        let map = InheritablePackageMap::from_toml_str(input).unwrap();
        let pool = pool(&[(
            "boltons",
            "{ version = \">=24\", channel = \"conda-forge\" }",
        )]);
        let resolved = map.resolve(&pool, true).unwrap();
        let spec = resolved
            .specs
            .get(&PackageName::from_str("boltons").unwrap())
            .unwrap();
        match spec {
            PixiSpec::Detailed(detailed) => {
                assert_eq!(detailed.version.as_ref().unwrap().to_string(), ">=24");
                assert!(
                    detailed.channel.is_some(),
                    "channel from workspace preserved"
                );
                assert!(detailed.build.is_some(), "build override applied");
            }
            other => panic!("expected DetailedVersion, got {other:?}"),
        }
    }

    #[test]
    fn member_keeps_full_control_when_writing_direct_spec() {
        let input = r#"numpy = "2.0""#;
        let map = InheritablePackageMap::from_toml_str(input).unwrap();
        // The workspace pool defines an unrelated version, member should win.
        let pool = pool(&[("numpy", "\"1.*\"")]);
        let resolved = map.resolve(&pool, true).unwrap();
        let spec = resolved
            .specs
            .get(&PackageName::from_str("numpy").unwrap())
            .unwrap();
        assert_eq!(spec.as_version_spec().unwrap().to_string(), "==2.0");
    }
}
