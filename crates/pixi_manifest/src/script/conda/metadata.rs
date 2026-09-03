use std::str::FromStr;

use indexmap::IndexMap;
use itertools::Itertools;
use pixi_spec::PixiSpec;
use pixi_toml::{TomlFromStr, TomlWith, custom_error, custom_error_message_with_help};
use rattler_conda_types::{NamedChannelOrUrl, PackageName, VersionSpec};
use toml_span::{
    DeserError, ErrorKind, Span, Spanned, Value,
    de_helpers::{TableHelper, expected},
    value::{Key, Table, ValueInner},
};

use super::entrypoint::Entrypoint;
use crate::{
    ManifestKind, PackageDependencySpec, PrioritizedChannel, TomlError,
    discovery::{RequiresPixiCheck, check_requires_pixi_early},
    script::tool_pixi::{ScriptKind, validate_tool_pixi},
    toml::{TomlManifest, TomlWorkspace},
    utils::{
        PixiSpanned,
        inheritable_package_map::{InheritablePackageMap, InheritableSpec},
        package_map::DependencyTable,
    },
};

/// The dependency keys the `conda-script` specification allows.
const ALLOWED_DEPENDENCY_KEYS: &[&str] = &[
    "version",
    "build",
    "build-number",
    "channel",
    "subdir",
    "extras",
    "flags",
    "md5",
    "sha256",
    "url",
    "when",
];

/// The keys that may accompany `url`; the URL already determines the
/// artifact, so matchspec fields make no sense next to it. The specification
/// also allows `when`, which pixi cannot attach to a URL spec yet.
const URL_COMPATIBLE_KEYS: &[&str] = &["url", "md5", "sha256"];

/// The fields of a `conda-script` block that the specification defines.
#[derive(Debug, Clone)]
pub struct CondaScriptMetadata {
    /// The conda channels the dependencies are solved from.
    pub channels: Vec<NamedChannelOrUrl>,
    /// The command that runs the script.
    pub entrypoint: Entrypoint,
    /// The conda dependencies declared in `[dependencies]`.
    pub dependencies: IndexMap<PackageName, PixiSpec>,
}

/// A block parsed into the specification's fields and the manifest pixi
/// builds the environment from.
pub(crate) struct ParsedBlock {
    pub(crate) metadata: CondaScriptMetadata,
    pub(crate) requires_pixi: RequiresPixiCheck,
    /// `[dependencies]` in the shape of a manifest dependency table, with
    /// spans pointing into the block.
    pub(crate) dependencies: Option<PixiSpanned<DependencyTable>>,
    /// `tool.pixi` parsed as a manifest whose workspace carries the block's
    /// channels.
    pub(crate) manifest: TomlManifest,
}

impl ParsedBlock {
    pub(crate) fn from_toml_str(text: &str) -> Result<Self, TomlError> {
        let mut value = toml_span::parse(text)?;
        let requires_pixi = check_requires_pixi_early(&value, ManifestKind::CondaScript);
        let mut th = TableHelper::new(&mut value)?;
        let mut errors = DeserError { errors: Vec::new() };

        let channels = th
            .required_s::<TomlWith<Vec<Spanned<NamedChannelOrUrl>>, Vec<Spanned<TomlFromStr<_>>>>>(
                "channels",
            )
            .ok()
            .map(|channels| Spanned {
                span: channels.span,
                value: TomlWith::into_inner(channels.value),
            });
        let entrypoint = th.required::<Entrypoint>("entrypoint").ok();

        let dependencies = match th.take("dependencies") {
            Some((_, mut value)) => {
                let span = value.span;
                match dependency_table(&mut value) {
                    Ok(dependencies) => Some((span, dependencies)),
                    Err(table_errors) => {
                        errors.merge(table_errors);
                        None
                    }
                }
            }
            None => None,
        };

        let pixi = match th.take("tool") {
            Some((_, mut tool)) => match pixi_tool_value(&mut tool) {
                Ok(pixi) => pixi,
                Err(tool_errors) => {
                    errors.merge(tool_errors);
                    None
                }
            },
            None => None,
        };
        let pixi = pixi.unwrap_or_else(|| Value::new(ValueInner::Table(Table::new())));

        if let Err(finalize_errors) = th.finalize(None) {
            errors.merge(finalize_errors);
        }

        if let Some(channels) = &channels {
            if channels.value.is_empty() {
                errors.errors.push(custom_error(
                    "`channels` must list at least one channel",
                    channels.span,
                ));
            }
            for channel in &channels.value {
                if channel.value.as_str().trim().is_empty() {
                    errors
                        .errors
                        .push(custom_error("a channel must not be empty", channel.span));
                }
            }
        }

        if !errors.errors.is_empty() {
            return Err(errors.into());
        }
        validate_tool_pixi(&pixi, ScriptKind::CondaScript)?;

        let channels: Vec<NamedChannelOrUrl> = channels
            .expect("missing channels were reported above")
            .value
            .into_iter()
            .map(|channel| channel.value)
            .collect();
        let manifest = tool_pixi_manifest(pixi, &channels).map_err(TomlError::from)?;
        let (dependencies, specs) = match dependencies {
            Some((span, dependencies)) => (
                Some(PixiSpanned {
                    span: Some(span.start..span.end),
                    value: dependencies.table,
                }),
                dependencies.specs,
            ),
            None => (None, IndexMap::new()),
        };

        Ok(Self {
            requires_pixi,
            metadata: CondaScriptMetadata {
                channels,
                entrypoint: entrypoint.expect("a missing entrypoint was reported above"),
                dependencies: specs,
            },
            dependencies,
            manifest,
        })
    }
}

/// The block's `[dependencies]`, both as plain specs and as a manifest
/// dependency table.
struct BlockDependencies {
    specs: IndexMap<PackageName, PixiSpec>,
    table: DependencyTable,
}

/// Parses `[dependencies]`, rejecting names that collide after
/// normalization.
fn dependency_table(value: &mut Value<'_>) -> Result<BlockDependencies, DeserError> {
    let table = match value.take() {
        ValueInner::Table(table) => table,
        inner => return Err(expected("a table", inner, value.span).into()),
    };

    let mut errors = DeserError { errors: Vec::new() };
    let mut specs = IndexMap::new();
    let mut manifest_table = InheritablePackageMap::default();
    for (key, mut value) in table.into_iter().sorted_by_key(|(key, _)| key.span.start) {
        if key.name.is_empty() {
            errors.errors.push(custom_error(
                "a dependency name must not be empty",
                key.span,
            ));
            continue;
        }
        let name = match PackageName::from_str(&key.name) {
            Ok(name) => name,
            Err(_) => {
                errors.errors.push(custom_error(
                    custom_error_message_with_help(
                        &format!("`{}` is not a package name", key.name.escape_debug()),
                        "package names consist of letters, digits, `-`, `_` and `.`",
                    ),
                    key.span,
                ));
                continue;
            }
        };
        if let Some(first) = manifest_table.name_spans.get(&name) {
            errors.errors.push(toml_span::Error {
                kind: ErrorKind::DuplicateKey {
                    key: key.name.into_owned(),
                    first: Span::new(first.start, first.end),
                },
                span: key.span,
                line_info: None,
            });
            continue;
        }
        let value_span = value.span;
        match conda_script_spec(&mut value) {
            Ok(spec) => {
                manifest_table.specs.insert(
                    name.clone(),
                    InheritableSpec::Direct(Box::new(PackageDependencySpec::from(spec.clone()))),
                );
                manifest_table
                    .name_spans
                    .insert(name.clone(), key.span.start..key.span.end);
                manifest_table
                    .value_spans
                    .insert(name.clone(), value_span.start..value_span.end);
                specs.insert(name, spec);
            }
            Err(spec_errors) => errors.merge(spec_errors),
        }
    }

    if errors.errors.is_empty() {
        Ok(BlockDependencies {
            specs,
            table: DependencyTable {
                specs: manifest_table,
                ..DependencyTable::default()
            },
        })
    } else {
        Err(errors)
    }
}

/// Parses one `[dependencies]` value, restricted to the keys the
/// `conda-script` specification defines.
fn conda_script_spec(value: &mut Value<'_>) -> Result<PixiSpec, DeserError> {
    let span = value.span;
    if let Some(table) = value.as_table() {
        let unknown: Vec<_> = table
            .keys()
            .filter(|key| !ALLOWED_DEPENDENCY_KEYS.contains(&key.name.as_ref()))
            .map(|key| (key.name.to_string(), key.span))
            .collect();
        if !unknown.is_empty() {
            return Err(toml_span::Error {
                kind: ErrorKind::UnexpectedKeys {
                    keys: unknown,
                    expected: ALLOWED_DEPENDENCY_KEYS
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                },
                span,
                line_info: None,
            }
            .into());
        }

        if table.keys().any(|key| key.name == "url") {
            let mut errors = DeserError { errors: Vec::new() };
            for key in table
                .keys()
                .filter(|key| !URL_COMPATIBLE_KEYS.contains(&key.name.as_ref()))
            {
                errors.errors.push(custom_error(
                    if key.name == "when" {
                        custom_error_message_with_help(
                            "pixi cannot combine `url` with `when` yet",
                            "the specification allows it, but pixi does not support conditions on URL specs so far",
                        )
                    } else {
                        custom_error_message_with_help(
                            &format!("`url` cannot be combined with `{}`", key.name),
                            "the URL already determines the artifact; only `md5` and `sha256` may accompany it",
                        )
                    },
                    key.span,
                ));
            }
            if !errors.errors.is_empty() {
                return Err(errors);
            }
        }

        // Every key is optional and `version` defaults to `*`.
        if table.is_empty() {
            return Ok(PixiSpec::from(VersionSpec::Any));
        }
    }

    let spec = <PixiSpec as toml_span::Deserialize>::deserialize(value)?;
    if spec.is_source() {
        return Err(custom_error(
            custom_error_message_with_help(
                "`url` must point at a conda package archive",
                "source dependencies are not part of the conda-script specification; declare them under `[tool.pixi.dependencies]`",
            ),
            span,
        )
        .into());
    }
    Ok(spec)
}

/// Takes `pixi` out of the `[tool]` table; every other tool's table is
/// ignored without looking inside it.
fn pixi_tool_value<'de>(tool: &mut Value<'de>) -> Result<Option<Value<'de>>, DeserError> {
    let table = match tool.take() {
        ValueInner::Table(table) => table,
        inner => return Err(expected("a table", inner, tool.span).into()),
    };
    Ok(table
        .into_iter()
        .find(|(key, _)| key.name == "pixi")
        .map(|(_, pixi)| pixi))
}

/// Parses `tool.pixi` with the manifest parser and gives its workspace the
/// block's channels.
///
/// The workspace parser requires `channels`, which a block declares at its
/// top level instead; a placeholder satisfies the parser until the block's
/// channels replace it.
fn tool_pixi_manifest(
    mut pixi: Value<'_>,
    channels: &[NamedChannelOrUrl],
) -> Result<TomlManifest, DeserError> {
    if let ValueInner::Table(mut table) = pixi.take() {
        if let Some(workspace) = table.get_mut("workspace") {
            match workspace.take() {
                ValueInner::Table(mut workspace_table) => {
                    workspace_table.insert(
                        Key {
                            name: "channels".into(),
                            span: Span::default(),
                        },
                        Value::new(ValueInner::Array(Vec::new())),
                    );
                    workspace.set(ValueInner::Table(workspace_table));
                }
                other => workspace.set(other),
            }
        }
        pixi.set(ValueInner::Table(table));
    }

    let mut manifest = <TomlManifest as toml_span::Deserialize>::deserialize(&mut pixi)?;
    let workspace = manifest.workspace.get_or_insert_with(|| PixiSpanned {
        span: None,
        value: TomlWorkspace::default(),
    });
    workspace.value.channels = channels
        .iter()
        .cloned()
        .map(PrioritizedChannel::from)
        .collect();
    Ok(manifest)
}
