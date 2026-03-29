use std::{collections::HashMap, path::Path};

use miette::{Context, IntoDiagnostic};
use pixi_manifest::{ExcludeNewer, FeaturesExt};
use rattler_conda_types::ParseChannelError;
use serde_yaml::{Mapping, Value};

use crate::{
    Workspace,
    workspace::{Environment, HasWorkspaceRef, grouped_environment::GroupedEnvironment},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockFileChannel {
    pub url: String,
    pub exclude_newer: Option<ExcludeNewer>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LockFileChannelMetadata {
    environments: HashMap<String, Vec<LockFileChannel>>,
}

impl LockFileChannelMetadata {
    pub(crate) fn from_path_lossy(path: &Path) -> Self {
        let Ok(contents) = fs_err::read_to_string(path) else {
            return Self::default();
        };

        Self::from_yaml_str_lossy(&contents)
    }

    pub(crate) fn environment(&self, name: &str) -> Option<&[LockFileChannel]> {
        self.environments.get(name).map(Vec::as_slice)
    }

    fn from_yaml_str_lossy(contents: &str) -> Self {
        let Ok(document) = serde_yaml::from_str::<Value>(contents) else {
            return Self::default();
        };

        let Some(root) = document.as_mapping() else {
            return Self::default();
        };
        let Some(environments) = mapping_get(root, "environments").and_then(Value::as_mapping)
        else {
            return Self::default();
        };

        let environments = environments
            .iter()
            .filter_map(|(name, environment)| {
                let name = name.as_str()?;
                let channels = mapping_get(environment.as_mapping()?, "channels")?
                    .as_sequence()?
                    .iter()
                    .filter_map(parse_lock_file_channel)
                    .collect::<Vec<_>>();
                Some((name.to_string(), channels))
            })
            .collect();

        Self { environments }
    }
}

pub(crate) fn expected_lock_file_channels(
    environment: &Environment<'_>,
) -> Result<Vec<LockFileChannel>, ParseChannelError> {
    let grouped_environment = GroupedEnvironment::from(environment.clone());
    let channel_config = environment.workspace().channel_config();

    grouped_environment
        .prioritized_channels()
        .into_iter()
        .map(|channel| {
            Ok(LockFileChannel {
                url: channel
                    .channel
                    .clone()
                    .into_base_url(&channel_config)?
                    .to_string(),
                exclude_newer: channel.exclude_newer,
            })
        })
        .collect()
}

pub(crate) fn persist_channel_exclude_newer(
    path: &Path,
    workspace: &Workspace,
) -> miette::Result<()> {
    let expected_channels = workspace
        .environments()
        .into_iter()
        .map(|environment| {
            expected_lock_file_channels(&environment)
                .map(|channels| (environment.name().to_string(), channels))
        })
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    if expected_channels.iter().all(|(_, channels)| {
        channels
            .iter()
            .all(|channel| channel.exclude_newer.is_none())
    }) {
        return Ok(());
    }

    let contents = fs_err::read_to_string(path).into_diagnostic()?;
    let mut document: Value = serde_yaml::from_str(&contents)
        .into_diagnostic()
        .context("failed to parse lock-file yaml while persisting channel exclude-newer")?;

    let Some(root) = document.as_mapping_mut() else {
        return Ok(());
    };
    let Some(environments) = mapping_get_mut(root, "environments").and_then(Value::as_mapping_mut)
    else {
        return Ok(());
    };

    for (environment_name, channels) in expected_channels {
        let Some(environment) =
            mapping_get_mut(environments, &environment_name).and_then(Value::as_mapping_mut)
        else {
            continue;
        };
        let Some(locked_channels) =
            mapping_get_mut(environment, "channels").and_then(Value::as_sequence_mut)
        else {
            continue;
        };

        for (locked_channel, expected_channel) in locked_channels.iter_mut().zip(channels.iter()) {
            let Some(channel_mapping) = locked_channel.as_mapping_mut() else {
                continue;
            };

            let exclude_newer_key = string_value("exclude-newer");
            match expected_channel.exclude_newer {
                Some(exclude_newer) => {
                    channel_mapping.insert(
                        exclude_newer_key,
                        Value::String(format_lock_file_exclude_newer(exclude_newer)),
                    );
                }
                None => {
                    channel_mapping.remove(&exclude_newer_key);
                }
            }
        }
    }

    let rendered = serde_yaml::to_string(&document)
        .into_diagnostic()
        .context("failed to serialize lock-file yaml with channel exclude-newer")?;
    fs_err::write(path, rendered)
        .into_diagnostic()
        .context("failed to write lock-file with channel exclude-newer")?;

    Ok(())
}

fn parse_lock_file_channel(value: &Value) -> Option<LockFileChannel> {
    let mapping = value.as_mapping()?;
    let url = mapping_get(mapping, "url")?.as_str()?.to_string();
    let exclude_newer = mapping_get(mapping, "exclude-newer")
        .and_then(Value::as_str)
        .and_then(|value| value.parse().ok());

    Some(LockFileChannel { url, exclude_newer })
}

fn mapping_get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    let key = string_value(key);
    mapping.get(&key)
}

fn mapping_get_mut<'a>(mapping: &'a mut Mapping, key: &str) -> Option<&'a mut Value> {
    let key = string_value(key);
    mapping.get_mut(&key)
}

fn string_value(value: &str) -> Value {
    Value::String(value.to_string())
}

fn format_lock_file_exclude_newer(exclude_newer: ExcludeNewer) -> String {
    match exclude_newer {
        ExcludeNewer::Duration(duration) if duration.is_zero() => "0d".to_string(),
        _ => exclude_newer.to_string(),
    }
}
