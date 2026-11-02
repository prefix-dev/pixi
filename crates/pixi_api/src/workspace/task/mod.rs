use std::{
    collections::{HashMap, HashSet},
    error::Error,
};

use fancy_display::FancyDisplay;
use miette::Diagnostic;
use pixi_core::{
    Workspace,
    workspace::{
        Environment, EnvironmentFromNameError, WorkspaceMut,
        virtual_packages::{EnvironmentRunnability, classify_environment_runnability},
    },
};
use pixi_manifest::{
    EnvironmentName, FeatureName, GetFeatureError, HasWorkspaceManifest, LoadManifestsError,
    PixiPlatform, PixiPlatformName, PlatformEditError, Task, TaskEditError, TaskName,
};
use thiserror::Error;

use crate::interface::Interface;
use crate::workspace::{
    platforms::resolve_platforms,
    workspace::{SaveManifestError, save},
};

#[derive(Debug, Error, Diagnostic)]
pub enum TaskError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    LoadWorkspace(#[from] LoadManifestsError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    EnvironmentFromName(#[from] EnvironmentFromNameError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Platform(#[from] PlatformEditError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Edit(#[from] TaskEditError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Feature(#[from] GetFeatureError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Save(#[from] SaveManifestError),
}

/// Resolve `name` the same way the dependency CLI does: look it up in the
/// workspace; if it's not declared, accept it as a bare conda subdir and
/// return a fresh [`PixiPlatform::from_subdir`]. Returns `Ok(None)` for an
/// unset flag and an error only when neither the lookup nor the subdir
/// parse succeeds. The returned platform is *not* added to the workspace
/// here -- the caller decides whether to auto-declare it (the way
/// `task add` / `task alias` do) or leave the manifest alone (`task remove`).
fn resolve_task_platform(
    workspace: &Workspace,
    name: Option<&PixiPlatformName>,
) -> Result<Option<PixiPlatform>, PlatformEditError> {
    let Some(name) = name else { return Ok(None) };
    let workspace_platforms = workspace.workspace_manifest().workspace.platforms.clone();
    Ok(
        resolve_platforms(&workspace_platforms, std::slice::from_ref(name))?
            .into_iter()
            .next(),
    )
}

/// Resolve `platform`, auto-declare it on the default feature when it's a
/// subdir pixi hasn't seen yet (matching `pixi add --platform`), then add the
/// task under `feature`. Shared by `add_task`/`alias_task` so their resolution
/// rules stay in lock-step. Does not save the workspace.
fn declare_platform_and_add_task(
    workspace: &mut WorkspaceMut,
    name: &TaskName,
    task: &Task,
    feature: &FeatureName,
    platform: Option<&PixiPlatformName>,
) -> Result<(), TaskError> {
    let pixi_platform = resolve_task_platform(workspace.workspace(), platform)?;
    // The auto-declare is idempotent on already-declared entries.
    if let Some(p) = &pixi_platform {
        workspace
            .manifest()
            .add_platforms(std::slice::from_ref(p).iter(), &FeatureName::Default)?;
    }
    workspace
        .manifest()
        .add_task(name.clone(), task.clone(), pixi_platform.as_ref(), feature)?;
    Ok(())
}

pub async fn list_tasks<I: Interface>(
    interface: &I,
    workspace: &Workspace,
    environment: Option<EnvironmentName>,
) -> Result<HashMap<EnvironmentName, (EnvironmentRunnability, HashMap<TaskName, Task>)>, TaskError>
{
    let explicit_environment = environment
        .map(|n| {
            workspace
                .environment(&n)
                .ok_or(EnvironmentFromNameError::Unknown(n))
        })
        .transpose()?;

    // The lock file only refines the runnability of environments the machine
    // doesn't declare, so a broken one shouldn't stop the listing.
    let lock_file = match workspace.load_lock_file().await {
        Ok(result) => Some(result.into_lock_file_or_empty_with_warning()),
        Err(err) => {
            let mut message = err.to_string();
            let mut source = err.source();
            while let Some(cause) = source {
                message.push_str(&format!(": {cause}"));
                source = cause.source();
            }
            interface.warning(&message).await;
            None
        }
    };

    let env_task_map: HashMap<Environment, (EnvironmentRunnability, HashSet<TaskName>)> =
        if let Some(explicit_environment) = explicit_environment {
            let runnability =
                classify_environment_runnability(&explicit_environment, lock_file.as_ref());
            HashMap::from([(
                explicit_environment.clone(),
                (runnability, explicit_environment.get_filtered_tasks()),
            )])
        } else {
            workspace
                .environments()
                .iter()
                .map(|env| {
                    let runnability = classify_environment_runnability(env, lock_file.as_ref());
                    (env.clone(), (runnability, env.get_filtered_tasks()))
                })
                .collect()
        };

    Ok(env_task_map
        .into_iter()
        .map(|(env, (runnability, task_names))| {
            let env_name = env.name().clone();
            let best_declared_platform = env.best_declared_platform();
            let task_map = task_names
                .into_iter()
                .flat_map(|task_name| {
                    env.task(&task_name, best_declared_platform)
                        .ok()
                        .map(|task| (task_name, task.clone()))
                })
                .collect();
            (env_name, (runnability, task_map))
        })
        .collect())
}

pub async fn add_task<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    name: TaskName,
    task: Task,
    feature: FeatureName,
    platform: Option<PixiPlatformName>,
) -> Result<(), TaskError> {
    declare_platform_and_add_task(&mut workspace, &name, &task, &feature, platform.as_ref())?;
    save(workspace).await?;

    interface
        .success(&format!(
            "Added task `{}`: {}",
            name.fancy_display().bold(),
            task,
        ))
        .await;

    Ok(())
}

pub async fn alias_task<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    name: TaskName,
    task: Task,
    feature: FeatureName,
    platform: Option<PixiPlatformName>,
) -> Result<(), TaskError> {
    declare_platform_and_add_task(&mut workspace, &name, &task, &feature, platform.as_ref())?;
    save(workspace).await?;

    interface
        .success(&format!(
            "Added alias `{}`: {}",
            name.fancy_display().bold(),
            task,
        ))
        .await;

    Ok(())
}

pub async fn remove_tasks<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    names: Vec<TaskName>,
    platform: Option<PixiPlatformName>,
    feature: FeatureName,
) -> Result<(), TaskError> {
    let mut to_remove = Vec::new();

    // No auto-declare on removal: if the name doesn't match a declared
    // platform we still try the subdir fallback so users can target a
    // dep that lives under an as-yet-undeclared subdir, but we don't
    // mutate `[workspace].platforms` here. A miss surfaces as "task '...'
    // does not exist on '<name>'" below.
    let pixi_platform = resolve_task_platform(workspace.workspace(), platform.as_ref())?;

    for name in names.iter() {
        let declared = workspace.workspace().workspace.value.declares_task(
            name,
            pixi_platform.as_ref(),
            &feature,
        )?;
        if !declared {
            let message = match pixi_platform.as_ref() {
                Some(pixi_platform) => {
                    format!("task '{name}' does not exist on '{}'", pixi_platform.name())
                }
                None => format!("task '{name}' does not exist for {}", feature.user_facing()),
            };
            interface.error(&message).await;
            continue;
        }

        // Safe to remove
        to_remove.push(name);
    }

    // Nothing to remove, so leave the manifest alone.
    if to_remove.is_empty() {
        return Ok(());
    }

    let mut removed = Vec::with_capacity(to_remove.len());
    for name in to_remove {
        workspace
            .manifest()
            .remove_task(name.clone(), pixi_platform.as_ref(), &feature)?;
        removed.push(name);
    }

    save(workspace).await?;

    for name in removed {
        interface
            .success(&format!("Removed task `{}` ", name.fancy_display().bold()))
            .await;
    }

    Ok(())
}
