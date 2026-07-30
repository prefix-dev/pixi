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

/// Whether this machine can still install the default environment.
///
/// A platform edit can drop -- or retarget -- the only platform the host is
/// able to run. Both the manifest change and the lock-file refresh remain
/// valid in that case; only materialising a prefix here is impossible.
fn host_can_install(workspace: &Workspace) -> bool {
    workspace
        .default_environment()
        .best_declared_platform()
        .is_some()
}

/// Tell the user why nothing was installed after a platform change left the
/// workspace without a platform this machine can run.
async fn report_skipped_install<I: Interface>(interface: &I, workspace: &Workspace) {
    let platforms = workspace
        .workspace_manifest()
        .workspace
        .platforms
        .iter()
        .map(|p| p.name().as_str())
        .collect::<Vec<_>>();
    let message = if platforms.is_empty() {
        "Skipped installing the environment. The workspace no longer declares any platform."
            .to_string()
    } else {
        format!(
            "Skipped installing the environment. This machine can't run any of the remaining platforms: {}.",
            platforms.join(", ")
        )
    };
    interface.warning(&message).await;
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

    // Failing here would strand the manifest edit while the refreshed lock
    // file is already on disk, so skip the install rather than the edit.
    let skipped_install = !no_install && !host_can_install(workspace.workspace());

    get_update_lock_file_and_prefix(
        &workspace.workspace().default_environment(),
        None,
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: no_install || skipped_install,
            max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
            ..Default::default()
        },
        ReinstallPackages::default(),
        &InstallFilter::default(),
    )
    .await?;
    let workspace = workspace.save().await.into_diagnostic()?;

    interface.success(&format!("Updated platform {name}")).await;
    if skipped_install {
        report_skipped_install(interface, &workspace).await;
    }
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
    feature_name: FeatureName,
) -> miette::Result<()> {
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
    feature_name: FeatureName,
) -> miette::Result<()> {
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
                if feature_name.is_default() {
                    platform.to_string()
                } else {
                    format!("{platform} to {}", feature_name.user_facing())
                }
            )
        } else {
            format!(
                "Platform {} is already present; nothing to do",
                if feature_name.is_default() {
                    platform.to_string()
                } else {
                    format!("{platform} in {}", feature_name.user_facing())
                }
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
    feature_name: FeatureName,
) -> miette::Result<()> {
    // Remove the platform(s) from the manifest
    workspace
        .manifest()
        .remove_platforms(platforms.iter(), &feature_name)?;

    // Failing here would strand the removal while the refreshed lock file is
    // already on disk, so skip the install rather than the removal.
    let skipped_install = !no_install && !host_can_install(workspace.workspace());

    get_update_lock_file_and_prefix(
        &workspace.workspace().default_environment(),
        None,
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: no_install || skipped_install,
            max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
            ..Default::default()
        },
        ReinstallPackages::default(),
        &InstallFilter::default(),
    )
    .await?;
    let workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    for platform in platforms {
        interface
            .success(&format!(
                "Removed {}",
                if feature_name.is_default() {
                    platform.to_string()
                } else {
                    format!("{platform} from {}", feature_name.user_facing())
                }
            ))
            .await;
    }
    if skipped_install {
        report_skipped_install(interface, &workspace).await;
    }

    Ok(())
}
