use std::collections::HashMap;

use miette::IntoDiagnostic;
use pixi_core::{
    InstallFilter, UpdateLockFileOptions, Workspace,
    environment::{LockFileUsage, get_update_lock_file_and_prefix},
    lock_file::{ReinstallPackages, UpdateMode},
    workspace::WorkspaceMut,
};
use pixi_manifest::FeaturesExt;
use pixi_manifest::{EnvironmentName, FeatureName, PrioritizedChannel};
use rattler_conda_types::{ChannelConfig, NamedChannelOrUrl};
use serde::{Deserialize, Serialize};

use crate::Interface;

#[derive(Deserialize, Serialize, Debug, Default)]
pub struct ChannelOptions {
    pub channel: Vec<NamedChannelOrUrl>,
    pub priority: Option<i32>,
    pub prepend: bool,
    pub feature: Option<String>,
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
) -> miette::Result<()> {
    // Add the channels to the manifest
    workspace.manifest().add_channels(
        prioritized_channels(&options.channel, options.priority),
        &feature_name(&options.feature),
        options.prepend,
    )?;

    // TODO: Update all environments touched by the features defined.
    get_update_lock_file_and_prefix(
        &workspace.workspace().default_environment(),
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: options.lock_file_usage,
            no_install: options.no_install,
            max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
        },
        ReinstallPackages::default(),
        &InstallFilter::default(),
    )
    .await?;

    let workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    report(
        interface,
        &options.channel,
        options.priority,
        "Added",
        &workspace.channel_config(),
    )
    .await?;

    Ok(())
}

pub async fn remove<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    options: ChannelOptions,
) -> miette::Result<()> {
    // Remove the channels from the manifest
    workspace.manifest().remove_channels(
        prioritized_channels(&options.channel, options.priority),
        &feature_name(&options.feature),
    )?;

    // Try to update the lock-file without the removed channels
    get_update_lock_file_and_prefix(
        &workspace.workspace().default_environment(),
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: options.lock_file_usage,
            no_install: options.no_install,
            max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
        },
        ReinstallPackages::default(),
        &InstallFilter::default(),
    )
    .await?;
    let workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    report(
        interface,
        &options.channel,
        options.priority,
        "Removed",
        &workspace.channel_config(),
    )
    .await?;

    Ok(())
}

fn feature_name(feature: &Option<String>) -> FeatureName {
    feature
        .clone()
        .map_or_else(FeatureName::default, FeatureName::from)
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
    channel: &Vec<NamedChannelOrUrl>,
    priority: Option<i32>,
    operation: &str,
    channel_config: &ChannelConfig,
) -> miette::Result<()> {
    for channel in channel {
        match channel {
            NamedChannelOrUrl::Name(name) => {
                interface
                    .success(&format!(
                        "{operation} {} ({}){}",
                        name,
                        channel
                            .clone()
                            .into_base_url(channel_config)
                            .into_diagnostic()?,
                        priority.map_or_else(|| "".to_string(), |p| format!(" at priority {p}"))
                    ))
                    .await
            }
            NamedChannelOrUrl::Url(url) => {
                interface
                    .success(&format!(
                        "{operation} {}{}",
                        url,
                        priority.map_or_else(|| "".to_string(), |p| format!(" at priority {p}")),
                    ))
                    .await
            }
            NamedChannelOrUrl::Path(path) => {
                interface
                    .success(&format!(
                        "{}{operation} {}",
                        console::style(console::Emoji("✔ ", "")).green(),
                        path
                    ))
                    .await
            }
        }
    }
    Ok(())
}
