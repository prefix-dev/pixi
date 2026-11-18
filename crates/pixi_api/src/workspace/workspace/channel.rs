use std::collections::HashMap;

use miette::Diagnostic;
use pixi_core::{
    UpdateLockFileOptions, Workspace,
    environment::{LockFileAndPrefixesError, LockFileUsage, update_lock_file_after_edit},
    workspace::WorkspaceMut,
};
use pixi_manifest::FeaturesExt;
use pixi_manifest::{
    AddChannelOutcome, EnvironmentName, FeatureName, LoadManifestsError, PrioritizedChannel,
    RemoveChannelsError, TomlError,
};
use rattler_conda_types::{ChannelConfig, NamedChannelOrUrl, ParseChannelError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Interface;
use crate::workspace::workspace::{SaveManifestError, save};

#[derive(Debug, Error, Diagnostic)]
pub enum ChannelError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    LoadWorkspace(#[from] LoadManifestsError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(#[from] TomlError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    RemoveChannels(#[from] RemoveChannelsError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    LockFileAndPrefixes(#[from] LockFileAndPrefixesError),

    #[error("failed to resolve channel '{channel}'")]
    ResolveChannel {
        channel: String,
        #[source]
        source: ParseChannelError,
    },

    #[error(transparent)]
    #[diagnostic(transparent)]
    Save(#[from] SaveManifestError),
}

#[derive(Deserialize, Serialize, Debug, Default)]
pub struct ChannelOptions {
    pub channels: Vec<NamedChannelOrUrl>,
    pub feature: FeatureName,
    pub no_install: bool,
    pub lock_file_usage: LockFileUsage,
}

pub async fn list(workspace: &Workspace) -> HashMap<EnvironmentName, Vec<NamedChannelOrUrl>> {
    workspace
        .environments()
        .iter()
        .map(|env| {
            (
                env.name().clone(),
                env.channels().into_iter().cloned().collect(),
            )
        })
        .collect()
}

pub async fn add<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    options: ChannelOptions,
    priority: Option<i32>,
    prepend: bool,
) -> Result<(), ChannelError> {
    // Add the channels to the manifest
    let outcomes = workspace.manifest().add_channels(
        prioritized_channels(&options.channels, priority),
        &options.feature,
        prepend,
    )?;

    // TODO: Update all environments touched by the features defined.
    let update_options = UpdateLockFileOptions {
        lock_file_usage: options.lock_file_usage,
        no_install: options.no_install,
        max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
        ..Default::default()
    };
    update_lock_file_after_edit(
        workspace.workspace(),
        workspace.progress().cloned(),
        update_options,
    )
    .await?;

    let workspace = save(workspace).await?;

    // Report back to the user
    let channel_config = workspace.channel_config();
    let priority_suffix = priority_suffix(priority);
    for (channel, outcome) in outcomes {
        let channel = describe_channel(&channel, &channel_config)?;
        match outcome {
            AddChannelOutcome::Added => {
                interface
                    .success(&format!("Added {channel}{priority_suffix}"))
                    .await
            }
            AddChannelOutcome::Moved => {
                interface
                    .success(&format!("Moved {channel} to the front"))
                    .await
            }
            AddChannelOutcome::Updated => {
                interface
                    .success(&format!("Updated {channel}{priority_suffix}"))
                    .await
            }
            AddChannelOutcome::MovedAndUpdated => {
                interface
                    .success(&format!("Moved {channel} to the front{priority_suffix}"))
                    .await
            }
            AddChannelOutcome::Unchanged => {
                let state = if prepend {
                    "already at the front"
                } else {
                    "already listed"
                };
                interface.info(&format!("{channel} is {state}")).await
            }
        }
    }

    Ok(())
}

pub async fn remove<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    options: ChannelOptions,
    priority: Option<i32>,
) -> Result<(), ChannelError> {
    // Remove the channels from the manifest
    workspace.manifest().remove_channels(
        prioritized_channels(&options.channels, priority),
        &options.feature,
    )?;

    // Try to update the lock file without the removed channels
    let update_options = UpdateLockFileOptions {
        lock_file_usage: options.lock_file_usage,
        no_install: options.no_install,
        max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
        ..Default::default()
    };
    update_lock_file_after_edit(
        workspace.workspace(),
        workspace.progress().cloned(),
        update_options,
    )
    .await?;
    let workspace = save(workspace).await?;

    // Report back to the user
    report(
        interface,
        &options.channels,
        priority,
        "Removed",
        &workspace.channel_config(),
    )
    .await?;

    Ok(())
}

pub async fn set<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    options: ChannelOptions,
) -> Result<(), ChannelError> {
    // Set the channels in the manifest (this replaces all existing channels)
    workspace.manifest().set_channels(
        prioritized_channels(&options.channels, None),
        &options.feature,
    )?;

    // Update the lock file with the new channel configuration
    let update_options = UpdateLockFileOptions {
        lock_file_usage: options.lock_file_usage,
        no_install: options.no_install,
        max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
        ..Default::default()
    };
    update_lock_file_after_edit(
        workspace.workspace(),
        workspace.progress().cloned(),
        update_options,
    )
    .await?;

    let workspace = save(workspace).await?;

    // Report back to the user
    report(
        interface,
        &options.channels,
        None,
        "Using",
        &workspace.channel_config(),
    )
    .await?;

    Ok(())
}

fn prioritized_channels(
    channel: &[NamedChannelOrUrl],
    priority: Option<i32>,
) -> impl IntoIterator<Item = PrioritizedChannel> {
    channel
        .iter()
        .cloned()
        .map(move |channel| PrioritizedChannel::from((channel, priority)))
}

async fn report<I: Interface>(
    interface: &I,
    channels: &Vec<NamedChannelOrUrl>,
    priority: Option<i32>,
    operation: &str,
    channel_config: &ChannelConfig,
) -> Result<(), ChannelError> {
    for channel in channels {
        let message = format_channel_message(channel, priority, operation, channel_config)?;
        interface.success(&message).await;
    }
    Ok(())
}

fn format_channel_message(
    channel: &NamedChannelOrUrl,
    priority: Option<i32>,
    operation: &str,
    channel_config: &ChannelConfig,
) -> Result<String, ChannelError> {
    Ok(format!(
        "{operation} {}{}",
        describe_channel(channel, channel_config)?,
        priority_suffix(priority)
    ))
}

fn priority_suffix(priority: Option<i32>) -> String {
    priority.map_or_else(String::new, |p| format!(" at priority {p}"))
}

/// The channel as the user wrote it, followed by its URL for named channels.
fn describe_channel(
    channel: &NamedChannelOrUrl,
    channel_config: &ChannelConfig,
) -> Result<String, ChannelError> {
    Ok(match channel {
        NamedChannelOrUrl::Name(name) => {
            let base_url = channel
                .clone()
                .into_base_url(channel_config)
                .map_err(|source| ChannelError::ResolveChannel {
                    channel: name.clone(),
                    source,
                })?;
            format!("{name} ({base_url})")
        }
        NamedChannelOrUrl::Url(url) => url.to_string(),
        NamedChannelOrUrl::Path(path) => path.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn default_channel_config() -> ChannelConfig {
        ChannelConfig::default_with_root_dir(PathBuf::from("/"))
    }

    #[test]
    fn test_format_named_channel_without_priority() {
        let channel = NamedChannelOrUrl::Name("conda-forge".into());
        let result =
            format_channel_message(&channel, None, "Added", &default_channel_config()).unwrap();

        assert_eq!(
            result,
            "Added conda-forge (https://conda.anaconda.org/conda-forge/)"
        );
        assert!(!result.contains("at priority"));
    }

    #[test]
    fn test_format_named_channel_with_priority() {
        let channel = NamedChannelOrUrl::Name("conda-forge".into());
        let result =
            format_channel_message(&channel, Some(10), "Added", &default_channel_config()).unwrap();

        assert_eq!(
            result,
            "Added conda-forge (https://conda.anaconda.org/conda-forge/) at priority 10"
        );
    }
}
