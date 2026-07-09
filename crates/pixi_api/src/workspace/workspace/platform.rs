use miette::IntoDiagnostic;
use pixi_manifest::{
    EnvironmentName, FeatureName, HasWorkspaceManifest, PixiPlatform, PixiPlatformName,
    PlatformEdit, PlatformMove,
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

/// Reorder the workspace platform `name` relative to the others. Updates the
/// lockfile and saves the manifest.
pub async fn move_platform<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    name: PixiPlatformName,
    target: PlatformMove,
    no_install: bool,
) -> miette::Result<()> {
    workspace
        .manifest()
        .move_workspace_platform(&name, &target)?;

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

    interface.success(&format!("Moved platform {name}")).await;
    Ok(())
}

/// Outcome of [`add_auto_detected`], picked to tailor the report.
enum AutoDetectedOutcome {
    /// A new platform was added.
    Added,
    /// An existing platform with the same definition was reused.
    Adopted,
    /// The platform's name was already declared; nothing was inserted.
    AlreadyPresent,
}

/// Add the auto-detected platform for this machine, placed first so it wins
/// platform selection. `candidate` is the already-built detected platform
/// (name synthesised or user-given); `explicit_name` is whether the user
/// supplied a `name=` form, which decides whether a same-definition entry under
/// a different name is adopted or rejected. Updates the lockfile and saves the
/// manifest.
pub async fn add_auto_detected<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    candidate: PixiPlatform,
    explicit_name: bool,
    no_install: bool,
    feature: Option<String>,
) -> miette::Result<()> {
    let feature_name = feature.map_or_else(FeatureName::default, FeatureName::from);

    // Content-based dedup: an existing platform with the same definition *is*
    // this machine, regardless of name.
    let existing = workspace
        .workspace()
        .workspace_manifest()
        .workspace
        .platforms
        .iter()
        .find(|p| p.has_same_definition(&candidate))
        .cloned();

    let (name, outcome) = match existing {
        // Bare form, or an explicit name that already matches: adopt the
        // existing entry. Re-adding it is a workspace no-op (deduped by name)
        // but still registers feature membership when `--feature` is given.
        Some(existing) if !explicit_name || existing.name() == candidate.name() => {
            workspace
                .manifest()
                .add_platforms(std::iter::once(&existing), &feature_name)?;
            (existing.name().clone(), AutoDetectedOutcome::Adopted)
        }
        // No content match, or an explicit name conflicting with an existing
        // definition -- `add_platforms` rejects the latter with the shared
        // duplicate-definition error.
        _ => {
            let added = workspace
                .manifest()
                .add_platforms(std::iter::once(&candidate), &feature_name)?;
            let name = candidate.name().clone();
            let outcome = if added.iter().any(|p| p.name() == &name) {
                AutoDetectedOutcome::Added
            } else {
                AutoDetectedOutcome::AlreadyPresent
            };
            (name, outcome)
        }
    };

    // Order is selection priority: put the detected platform first.
    workspace
        .manifest()
        .move_workspace_platform(&name, &PlatformMove::ToTop)?;

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

    match outcome {
        AutoDetectedOutcome::Added => {
            interface
                .success(&format!(
                    "Added platform {name} (detected from this machine)"
                ))
                .await;
            interface.info(&auto_detected_hint(&name)).await;
        }
        AutoDetectedOutcome::Adopted => {
            interface
                .success(&format!(
                    "Platform {name} already matches this machine; moved it to the front"
                ))
                .await;
        }
        AutoDetectedOutcome::AlreadyPresent => {
            interface
                .success(&format!(
                    "Platform {name} is already present; moved it to the front"
                ))
                .await;
        }
    }

    Ok(())
}

/// Pointers shown after adding a fresh auto-detected platform: it is shared via
/// the manifest, it is usually more specific than needed, and `pixi info`
/// reveals which virtual packages are actually required.
fn auto_detected_hint(name: &PixiPlatformName) -> String {
    format!(
        "\n  This platform is written to pixi.toml and shared with everyone using the workspace.\n  \
         Auto-detection captures your machine exactly, which is often more specific than needed.\n\n  \
         After installing, `pixi info` shows each environment's \"Minimum platform\" -- the\n  \
         virtual packages actually required -- so you can see which ones are safe to drop.\n\n  \
         Refine it:\n    \
         pixi workspace platform edit {name} ...   # rename / drop virtual packages\n    \
         pixi workspace platform move {name} ...   # change its priority"
    )
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
