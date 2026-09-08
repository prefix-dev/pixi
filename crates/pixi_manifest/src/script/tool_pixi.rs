//! The subset of `tool.pixi` a script block may carry.
//!
//! A script represents one implicit default environment, so the tables that
//! shape a workspace with several environments, packages or tasks have no
//! meaning in it. Both block kinds share this allowlist; they only differ in
//! where channels are declared.

use itertools::Itertools;
use miette::{LabeledSpan, SourceSpan};
use toml_span::{Span, Value, value::Table};

use crate::{GenericError, TomlError};

/// The keys `tool.pixi` accepts in a script block.
const TOOL_PIXI_KEYS: &[&str] = &[
    "activation",
    "constraints",
    "dependencies",
    "exclude-newer",
    "pypi-dependencies",
    "pypi-exclude-newer",
    "target",
    "workspace",
];

/// The keys `tool.pixi.workspace` accepts in a script block, `channels`
/// aside.
const WORKSPACE_KEYS: &[&str] = &[
    "channel-priority",
    "conda-pypi-map",
    "exclude-newer",
    "platforms",
    "preview",
    "pypi-options",
    "requires-pixi",
    "solve-strategy",
];

/// The keys `tool.pixi.target.<selector>` accepts in a script block.
const TARGET_KEYS: &[&str] = &[
    "activation",
    "constraints",
    "dependencies",
    "pypi-dependencies",
];

const ONE_ENVIRONMENT: &str = "a script represents one implicit default environment";

/// The kind of block `tool.pixi` sits in, which decides where channels live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptKind {
    /// Channels are declared under `tool.pixi.workspace`.
    Pep723,
    /// Channels are declared at the top level of the block.
    CondaScript,
}

/// A key a script block cannot hold.
pub(crate) struct UnsupportedKey {
    path: String,
    span: Span,
    help: String,
}

impl UnsupportedKey {
    pub(crate) fn new(path: impl Into<String>, help: impl Into<String>, span: Span) -> Self {
        Self {
            path: path.into(),
            span,
            help: help.into(),
        }
    }
}

/// Rejects every key of `tool.pixi` outside the script subset, reporting
/// all of them in one diagnostic.
pub(crate) fn validate_tool_pixi(pixi: &Value<'_>, kind: ScriptKind) -> Result<(), TomlError> {
    finish(unsupported_tool_pixi_keys(pixi, kind)?)
}

/// The keys of `tool.pixi` outside the script subset.
///
/// Fails only when `tool.pixi` is not a table.
pub(crate) fn unsupported_tool_pixi_keys(
    pixi: &Value<'_>,
    kind: ScriptKind,
) -> Result<Vec<UnsupportedKey>, TomlError> {
    let Some(table) = pixi.as_table() else {
        return Err(GenericError::new("`tool.pixi` must be a table")
            .with_span(pixi.span.start..pixi.span.end)
            .into());
    };

    let mut keys = Vec::new();
    for (key, value) in table {
        let path = format!("tool.pixi.{}", key.name);
        match key.name.as_ref() {
            "system-requirements" => keys.push(UnsupportedKey::new(
                path,
                "a script without `platforms` resolves for the virtual packages of the machine it runs on; declare `platforms` with their virtual packages to pin a target instead",
                key.span,
            )),
            "workspace" => {
                if let Some(workspace) = value.as_table() {
                    keys.extend(unsupported_workspace_keys(workspace, &path, kind));
                }
            }
            "target" => {
                if let Some(targets) = value.as_table() {
                    for (selector, target) in targets {
                        if let Some(target) = target.as_table() {
                            let path = format!("{path}.{}", selector.name);
                            keys.extend(unsupported_keys(target, &path, TARGET_KEYS));
                        }
                    }
                }
            }
            name if TOOL_PIXI_KEYS.contains(&name) => {}
            _ => keys.push(UnsupportedKey::new(
                path,
                accepted_keys_help("tool.pixi", TOOL_PIXI_KEYS),
                key.span,
            )),
        }
    }
    Ok(keys)
}

fn unsupported_workspace_keys(
    workspace: &Table<'_>,
    path: &str,
    kind: ScriptKind,
) -> Vec<UnsupportedKey> {
    workspace
        .keys()
        .filter_map(|key| match key.name.as_ref() {
            "channels" => match kind {
                ScriptKind::Pep723 => None,
                ScriptKind::CondaScript => Some(UnsupportedKey::new(
                    format!("{path}.channels"),
                    "a conda-script block declares its channels at the top level",
                    key.span,
                )),
            },
            name if WORKSPACE_KEYS.contains(&name) => None,
            name => Some(UnsupportedKey::new(
                format!("{path}.{name}"),
                accepted_keys_help(path, WORKSPACE_KEYS),
                key.span,
            )),
        })
        .collect()
}

/// Every key of `table` outside `allowed`, reported with the path it was
/// found under.
pub(crate) fn unsupported_keys(
    table: &Table<'_>,
    path: &str,
    allowed: &[&str],
) -> Vec<UnsupportedKey> {
    table
        .keys()
        .filter(|key| !allowed.contains(&key.name.as_ref()))
        .map(|key| {
            UnsupportedKey::new(
                format!("{path}.{}", key.name),
                accepted_keys_help(path, allowed),
                key.span,
            )
        })
        .collect()
}

fn accepted_keys_help(table: &str, allowed: &[&str]) -> String {
    format!(
        "{ONE_ENVIRONMENT}; `{table}` accepts {}",
        allowed.iter().map(|key| format!("`{key}`")).join(", ")
    )
}

/// Turns the collected keys into one diagnostic that underlines each of
/// them in source order, with the help of every distinct reason.
pub(crate) fn finish(mut keys: Vec<UnsupportedKey>) -> Result<(), TomlError> {
    if keys.is_empty() {
        return Ok(());
    }
    keys.sort_by_key(|key| key.span.start);

    let paths = keys
        .iter()
        .map(|key| format!("`{}`", key.path))
        .collect_vec();
    let listed = match paths.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => unreachable!("the list of keys is not empty"),
    };
    let labels = keys.iter().map(|key| {
        LabeledSpan::new_primary_with_span(
            None,
            SourceSpan::new(key.span.start.into(), key.span.end - key.span.start),
        )
    });
    let help = keys.iter().map(|key| key.help.as_str()).unique().join("\n");

    Err(
        GenericError::new(format!("scripts do not support {listed}"))
            .with_labels(labels)
            .with_help(help)
            .into(),
    )
}
