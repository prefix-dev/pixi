use miette::IntoDiagnostic;
use pixi_manifest::{EnvironmentName, FeatureName};
use rattler_conda_types::Platform;
use std::collections::HashMap;

use pixi_core::{Workspace, workspace::WorkspaceMut};
use pixi_manifest::FeaturesExt;

use pixi_core::{
    UpdateLockFileOptions,
    environment::{InstallFilter, LockFileUsage, get_update_lock_file_and_prefix},
    lock_file::{ReinstallPackages, UpdateMode},
};

use crate::Interface;

pub async fn list(workspace: &Workspace) -> HashMap<EnvironmentName, Vec<Platform>> {
    workspace
        .environments()
        .iter()
        .map(|e| (e.name().clone(), e.platforms().into_iter().collect()))
        .collect()
}

pub async fn add<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    platforms: Vec<Platform>,
    no_install: bool,
    feature: Option<String>,
) -> miette::Result<()> {
    let feature_name = feature.map_or_else(FeatureName::default, FeatureName::from);

    // Add the platforms to the lock-file
    workspace
        .manifest()
        .add_platforms(platforms.iter(), &feature_name)?;

    // Try to update the lock-file with the new channels
    get_update_lock_file_and_prefix(
        &workspace.workspace().default_environment(),
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install,
            max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
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
                "Added {}",
                &feature_name.non_default().map_or_else(
                    || platform.to_string(),
                    |name| format!("{platform} to the feature {name}")
                )
            ))
            .await;
    }

    Ok(())
}

pub async fn remove<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    platforms: Vec<Platform>,
    no_install: bool,
    feature: Option<String>,
) -> miette::Result<()> {
    let feature_name = feature.map_or_else(FeatureName::default, FeatureName::from);

    // Remove the platform(s) from the manifest
    workspace
        .manifest()
        .remove_platforms(platforms.clone(), &feature_name)?;

    get_update_lock_file_and_prefix(
        &workspace.workspace().default_environment(),
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install,
            max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
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
                &feature_name.non_default().map_or_else(
                    || platform.to_string(),
                    |name| format!("{platform} from the feature {name}")
                )
            ))
            .await;
    }

    Ok(())
}
