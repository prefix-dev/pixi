use miette::IntoDiagnostic;
use pixi_manifest::{
    EnvironmentName, FeatureName, HasWorkspaceManifest, PixiPlatform, PixiPlatformName,
    PlatformEdit,
};
use std::collections::HashMap;

use pixi_core::{Workspace, workspace::WorkspaceMut};
use pixi_manifest::FeaturesExt;

use pixi_core::{
    UpdateLockFileOptions,
    environment::{InstallFilter, LockFileUsage, get_update_lock_file_and_prefix},
    lock_file::{ReinstallPackages, UpdateMode},
};

use crate::Interface;

pub async fn list(workspace: &Workspace) -> HashMap<EnvironmentName, Vec<PixiPlatformName>> {
    workspace
        .environments()
        .iter()
        .map(|e| (e.name().clone(), e.platforms().into_iter().collect()))
        .collect()
}

/// Look up the full [`PixiPlatform`] for `name` in the workspace manifest, or
/// `None` if no platform with that name is declared.
pub async fn get_workspace_platform(
    workspace: &Workspace,
    name: &PixiPlatformName,
) -> Option<PixiPlatform> {
    workspace
        .workspace_manifest()
        .workspace
        .platforms
        .iter()
        .find(|p| p.name() == name)
        .cloned()
}

/// Apply an edit to an existing workspace platform identified by `name`.
/// Updates the lockfile and saves the manifest.
pub async fn edit<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    name: PixiPlatformName,
    edit: PlatformEdit,
    no_install: bool,
) -> miette::Result<()> {
    workspace.manifest().edit_workspace_platform(&name, edit)?;

    get_update_lock_file_and_prefix(
        &workspace.workspace().default_environment(),
        None,
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install,
            max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
            ..Default::default()
        },
        ReinstallPackages::default(),
        &InstallFilter::default(),
    )
    .await?;
    workspace.save().await.into_diagnostic()?;

    interface.success(&format!("Updated platform {name}")).await;
    Ok(())
}

pub async fn add<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    platforms: Vec<PixiPlatform>,
    no_install: bool,
    feature: Option<String>,
) -> miette::Result<()> {
    let feature_name = feature.map_or_else(FeatureName::default, FeatureName::from);

    // Add the platforms to the manifest; `added` holds only those that caused
    // an actual change so already-declared platforms are reported as no-ops.
    let added = workspace
        .manifest()
        .add_platforms(platforms.iter(), &feature_name)?;

    // Try to update the lock file with the new channels
    get_update_lock_file_and_prefix(
        &workspace.workspace().default_environment(),
        None,
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install,
            max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
            ..Default::default()
        },
        ReinstallPackages::default(),
        &InstallFilter::default(),
    )
    .await?;
    workspace.save().await.into_diagnostic()?;

    // Report back to the user
    for platform in &platforms {
        let message = if added.contains(platform) {
            format!(
                "Added {}",
                feature_name.non_default().map_or_else(
                    || platform.to_string(),
                    |name| format!("{platform} to the feature {name}")
                )
            )
        } else {
            format!(
                "Platform {} is already present; nothing to do",
                feature_name.non_default().map_or_else(
                    || platform.to_string(),
                    |name| format!("{platform} in the feature {name}")
                )
            )
        };
        interface.success(&message).await;
    }

    Ok(())
}

pub async fn remove<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    platforms: Vec<PixiPlatform>,
    no_install: bool,
    feature: Option<String>,
) -> miette::Result<()> {
    let feature_name = feature.map_or_else(FeatureName::default, FeatureName::from);

    // Remove the platform(s) from the manifest
    workspace
        .manifest()
        .remove_platforms(platforms.iter(), &feature_name)?;

    get_update_lock_file_and_prefix(
        &workspace.workspace().default_environment(),
        None,
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install,
            max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
            ..Default::default()
        },
        ReinstallPackages::default(),
        &InstallFilter::default(),
    )
    .await?;
    workspace.save().await.into_diagnostic()?;

    // Report back to the user
    for platform in platforms {
        interface
            .success(&format!(
                "Removed {}",
                feature_name.non_default().map_or_else(
                    || platform.to_string(),
                    |name| format!("{platform} from the feature {name}")
                )
            ))
            .await;
    }

    Ok(())
}
