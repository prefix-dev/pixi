use std::{fmt::Display, hash::Hash, str::FromStr};

use indexmap::IndexMap;
use itertools::Itertools;
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use pixi_toml::{TomlFromStr, custom_error, custom_error_message_with_help};
use rattler_conda_types::{NamedChannelOrUrl, PackageName, VersionSpec};
use toml_span::{
    DeserError, ErrorKind, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

use super::entrypoint::Entrypoint;

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

/// The parsed content of a `conda-script` block.
#[derive(Debug, Clone)]
pub struct CondaScriptMetadata {
    /// The conda channels the dependencies are solved from.
    pub channels: Vec<NamedChannelOrUrl>,
    /// The command that runs the script.
    pub entrypoint: Entrypoint,
    /// The conda dependencies declared in `[dependencies]`.
    pub dependencies: IndexMap<PackageName, PixiSpec>,
    /// The pixi-specific configuration under `[tool.pixi]`.
    pub pixi: PixiTool,
}

/// The `[tool.pixi]` table of a `conda-script` block.
#[derive(Debug, Clone, Default)]
pub struct PixiTool {
    /// Conda dependencies in pixi's native spec syntax. They merge with the
    /// `[dependencies]` table the way pixi merges features: both specs reach
    /// the solver.
    pub dependencies: IndexMap<PackageName, PixiSpec>,
    /// PyPI packages added to the environment.
    pub pypi_dependencies: IndexMap<PypiPackageName, PixiPypiSpec>,
}

impl CondaScriptMetadata {
    pub(crate) fn from_toml_str(text: &str) -> Result<Self, DeserError> {
        let mut value = toml_span::parse(text)?;
        <Self as toml_span::Deserialize>::deserialize(&mut value)
    }
}

impl<'de> toml_span::Deserialize<'de> for CondaScriptMetadata {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let mut errors = DeserError { errors: Vec::new() };

        let channels = th
            .required_s::<Vec<TomlFromStr<NamedChannelOrUrl>>>("channels")
            .ok();
        let entrypoint = th.required::<Entrypoint>("entrypoint").ok();

        let dependencies = match th.take("dependencies") {
            Some((_, mut value)) => match dependency_table(&mut value, conda_script_spec) {
                Ok(dependencies) => dependencies,
                Err(table_errors) => {
                    errors.merge(table_errors);
                    IndexMap::new()
                }
            },
            None => IndexMap::new(),
        };

        let pixi = match th.take("tool") {
            Some((_, mut value)) => match tool_table(&mut value) {
                Ok(pixi) => pixi,
                Err(tool_errors) => {
                    errors.merge(tool_errors);
                    PixiTool::default()
                }
            },
            None => PixiTool::default(),
        };

        if let Err(finalize_errors) = th.finalize(None) {
            errors.merge(finalize_errors);
        }

        if let Some(channels) = &channels
            && channels.value.is_empty()
        {
            errors.errors.push(custom_error(
                "`channels` must list at least one channel",
                channels.span,
            ));
        }

        if !errors.errors.is_empty() {
            return Err(errors);
        }

        Ok(Self {
            channels: channels
                .expect("missing channels were reported above")
                .value
                .into_iter()
                .map(TomlFromStr::into_inner)
                .collect(),
            entrypoint: entrypoint.expect("a missing entrypoint was reported above"),
            dependencies,
            pixi,
        })
    }
}

/// Parses a `name = <spec>` table, rejecting names that collide after
/// normalization.
fn dependency_table<'de, Name, Spec>(
    value: &mut Value<'de>,
    parse_spec: impl Fn(&mut Value<'de>) -> Result<Spec, DeserError>,
) -> Result<IndexMap<Name, Spec>, DeserError>
where
    Name: FromStr + Hash + Eq + Clone,
    Name::Err: Display,
{
    let table = match value.take() {
        ValueInner::Table(table) => table,
        inner => return Err(expected("a table", inner, value.span).into()),
    };

    let mut errors = DeserError { errors: Vec::new() };
    let mut result: IndexMap<Name, Spec> = IndexMap::new();
    let mut name_spans: IndexMap<Name, toml_span::Span> = IndexMap::new();
    for (key, mut value) in table.into_iter().sorted_by_key(|(key, _)| key.span.start) {
        let name = match Name::from_str(&key.name) {
            Ok(name) => name,
            Err(error) => {
                errors
                    .errors
                    .push(custom_error(error.to_string(), key.span));
                continue;
            }
        };
        if let Some(first) = name_spans.get(&name) {
            errors.errors.push(toml_span::Error {
                kind: ErrorKind::DuplicateKey {
                    key: key.name.into_owned(),
                    first: *first,
                },
                span: key.span,
                line_info: None,
            });
            continue;
        }
        name_spans.insert(name.clone(), key.span);
        match parse_spec(&mut value) {
            Ok(spec) => {
                result.insert(name, spec);
            }
            Err(spec_errors) => errors.merge(spec_errors),
        }
    }

    if errors.errors.is_empty() {
        Ok(result)
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

/// Parses the `[tool]` table: `pixi` is interpreted, every other tool's
/// table is ignored without looking inside it.
fn tool_table(value: &mut Value<'_>) -> Result<PixiTool, DeserError> {
    let table = match value.take() {
        ValueInner::Table(table) => table,
        inner => return Err(expected("a table", inner, value.span).into()),
    };

    for (key, mut value) in table {
        if key.name == "pixi" {
            return pixi_tool_table(&mut value);
        }
    }
    Ok(PixiTool::default())
}

fn pixi_tool_table(value: &mut Value<'_>) -> Result<PixiTool, DeserError> {
    let mut th = TableHelper::new(value)?;
    let mut errors = DeserError { errors: Vec::new() };

    let dependencies = match th.take("dependencies") {
        Some((_, mut value)) => {
            match dependency_table(
                &mut value,
                <PixiSpec as toml_span::Deserialize>::deserialize,
            ) {
                Ok(dependencies) => dependencies,
                Err(table_errors) => {
                    errors.merge(table_errors);
                    IndexMap::new()
                }
            }
        }
        None => IndexMap::new(),
    };

    let pypi_dependencies = match th.take("pypi-dependencies") {
        Some((_, mut value)) => {
            match dependency_table(
                &mut value,
                <PixiPypiSpec as toml_span::Deserialize>::deserialize,
            ) {
                Ok(dependencies) => dependencies,
                Err(table_errors) => {
                    errors.merge(table_errors);
                    IndexMap::new()
                }
            }
        }
        None => IndexMap::new(),
    };

    // Put the unclaimed keys back so each can be reported as unsupported.
    if let Err(finalize_errors) = th.finalize(Some(value)) {
        errors.merge(finalize_errors);
    }
    if let Some(table) = value.as_table() {
        for key in table.keys().sorted_by_key(|key| key.span.start) {
            errors.errors.push(custom_error(
                custom_error_message_with_help(
                    &format!(
                        "conda-script blocks do not support `tool.pixi.{}`",
                        key.name
                    ),
                    "a script represents one implicit default environment",
                ),
                key.span,
            ));
        }
    }

    if errors.errors.is_empty() {
        Ok(PixiTool {
            dependencies,
            pypi_dependencies,
        })
    } else {
        Err(errors)
    }
}
