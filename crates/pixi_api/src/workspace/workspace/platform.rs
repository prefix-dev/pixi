use fancy_display::FancyDisplay;
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

/// Helper to update the lock file and environment prefix for platform commands.
///
/// The lock file is solved for every declared platform, but a prefix can only
/// be materialised for the current system. When none of the environment's
/// platforms match this machine, skip prefix installation.
async fn update_platform_lock_file_and_prefix(
    workspace: &Workspace,
    progress: Option<std::sync::Arc<pixi_reporters::TopLevelProgress>>,
    no_install: bool,
    lock_file_usage: LockFileUsage,
) -> miette::Result<()> {
    let default_env = workspace.default_environment();
    let no_install = no_install || default_env.best_declared_platform().is_none();
    if no_install && default_env.best_declared_platform().is_none() {
        tracing::info!(
            "Skipping prefix installation: no platform supported by environment '{}' matches the current system",
            default_env.name().fancy_display()
        );
    }

    get_update_lock_file_and_prefix(
        &default_env,
        progress,
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage,
            no_install,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
            ..Default::default()
        },
        ReinstallPackages::default(),
        &InstallFilter::default(),
    )
    .await?;
    Ok(())
}

/// Apply an edit to an existing workspace platform identified by `name`.
/// Updates the lockfile and saves the manifest.
pub async fn edit<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    name: PixiPlatformName,
    edit: PlatformEdit,
    no_install: bool,
    lock_file_usage: LockFileUsage,
) -> miette::Result<()> {
    workspace.manifest().edit_workspace_platform(&name, edit)?;

    update_platform_lock_file_and_prefix(
        workspace.workspace(),
        workspace.progress().cloned(),
        no_install,
        lock_file_usage,
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
    lock_file_usage: LockFileUsage,
) -> miette::Result<()> {
    workspace
        .manifest()
        .move_workspace_platform(&name, &target)?;

    update_platform_lock_file_and_prefix(
        workspace.workspace(),
        workspace.progress().cloned(),
        no_install,
        lock_file_usage,
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
    lock_file_usage: LockFileUsage,
) -> miette::Result<()> {
    // A script's implicit platforms are pixi's own guess, not a declaration,
    // so deduplicating against them would make this a guaranteed no-op.
    workspace.forget_implicit_script_platforms();

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

    update_platform_lock_file_and_prefix(
        workspace.workspace(),
        workspace.progress().cloned(),
        no_install,
        lock_file_usage,
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
/// reveals what the installed packages actually require.
fn auto_detected_hint(name: &PixiPlatformName) -> String {
    format!(
        "\n  This platform is written to pixi.toml and shared with everyone using the workspace.\n  \
         Auto-detection captures your machine exactly, which is often more specific than needed.\n\n  \
         After installing, `pixi info` shows each environment's \"Minimum platform\" -- what the\n  \
         installed packages actually require -- so you can see which ones are safe to drop.\n\n  \
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
    lock_file_usage: LockFileUsage,
) -> miette::Result<()> {
    // Add the platforms to the manifest; `added` holds only those that caused
    // an actual change so already-declared platforms are reported as no-ops.
    let added = workspace
        .manifest()
        .add_platforms(platforms.iter(), &feature_name)?;

    // Try to update the lock file with the new channels
    update_platform_lock_file_and_prefix(
        workspace.workspace(),
        workspace.progress().cloned(),
        no_install,
        lock_file_usage,
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
    lock_file_usage: LockFileUsage,
) -> miette::Result<()> {
    // Remove the platform(s) from the manifest
    workspace
        .manifest()
        .remove_platforms(platforms.iter(), &feature_name)?;

    update_platform_lock_file_and_prefix(
        workspace.workspace(),
        workspace.progress().cloned(),
        no_install,
        lock_file_usage,
    )
    .await?;
    workspace.save().await.into_diagnostic()?;

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

    Ok(())
}

#[cfg(test)]
mod tests {
    use pixi_core::{Workspace, environment::LockFileUsage};
    use pixi_manifest::{FeatureName, PixiPlatform};
    use rattler_conda_types::Platform;

    use super::*;

    struct MockInterface;

    impl Interface for MockInterface {
        async fn is_cli(&self) -> bool {
            false
        }
        async fn confirm(&self, _msg: &str) -> miette::Result<bool> {
            Ok(true)
        }
        async fn error(&self, _msg: &str) {}
        async fn info(&self, _msg: &str) {}
        async fn success(&self, _msg: &str) {}
        async fn warning(&self, _msg: &str) {}
    }

    fn workspace_from(toml: &str) -> (tempfile::TempDir, Workspace) {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("pixi.toml");
        fs_err::write(&path, toml).unwrap();
        let ws = Workspace::from_path(&path).expect("failed to load workspace");
        (tmp, ws)
    }

    #[tokio::test]
    async fn test_remove_host_platform_without_no_install() {
        let current_platform = Platform::current().expect("host platform");
        let host = current_platform.as_str();
        let (_tmp, workspace) = workspace_from(&format!(
            r#"
[workspace]
name = "test-remove-host"
channels = []
platforms = ["{host}"]
"#
        ));
        let platform = PixiPlatform::from_subdir(current_platform);
        let result = remove(
            &MockInterface,
            workspace.modify().unwrap(),
            vec![platform],
            false,
            FeatureName::Default,
            LockFileUsage::Update,
        )
        .await;

        assert!(
            result.is_ok(),
            "remove host platform failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_remove_sole_foreign_platform_without_no_install() {
        let (_tmp, workspace) = workspace_from(
            r#"
[workspace]
name = "test-platform-remove"
channels = []
platforms = ["win-64"]
"#,
        );
        let platform = PixiPlatform::from_subdir(Platform::Win64);
        let result = remove(
            &MockInterface,
            workspace.modify().unwrap(),
            vec![platform],
            false,
            FeatureName::Default,
            LockFileUsage::Update,
        )
        .await;

        assert!(
            result.is_ok(),
            "remove foreign platform failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_add_foreign_platform_without_no_install() {
        let (_tmp, workspace) = workspace_from(
            r#"
[workspace]
name = "test-platform-add"
channels = []
platforms = ["linux-64"]
"#,
        );
        let platform = PixiPlatform::from_subdir(Platform::Win64);
        let result = add(
            &MockInterface,
            workspace.modify().unwrap(),
            vec![platform],
            false,
            FeatureName::Default,
            LockFileUsage::Update,
        )
        .await;

        assert!(
            result.is_ok(),
            "add foreign platform failed: {:?}",
            result.err()
        );
    }
}
