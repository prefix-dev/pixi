use std::collections::{HashMap, HashSet};

use fancy_display::FancyDisplay;
use miette::IntoDiagnostic;
use pixi_core::{
    Workspace,
    workspace::{
        Environment, WorkspaceMut, virtual_packages::verify_current_platform_can_run_environment,
    },
};
use pixi_manifest::{
    EnvironmentName, FeatureName, HasWorkspaceManifest, PixiPlatform, PixiPlatformName, Task,
    TaskName,
};

use crate::interface::Interface;

/// Look up a [`PixiPlatform`] from the workspace by name. Returns `Ok(None)`
/// when `name` is `None`, the looked-up platform when known, or an error if
/// the workspace does not define a platform with that name.
fn lookup_platform<'p>(
    workspace: &'p pixi_core::Workspace,
    name: Option<&PixiPlatformName>,
) -> miette::Result<Option<&'p PixiPlatform>> {
    let Some(name) = name else { return Ok(None) };
    workspace
        .workspace_manifest()
        .workspace
        .platform_by_name(name)
        .map(Some)
        .ok_or_else(|| miette::miette!("workspace does not define a platform named '{name}'"))
}

pub async fn list_tasks(
    workspace: &Workspace,
    environment: Option<EnvironmentName>,
) -> miette::Result<HashMap<EnvironmentName, HashMap<TaskName, Task>>> {
    let explicit_environment = environment
        .map(|n| {
            workspace
                .environment(&n)
                .ok_or_else(|| miette::miette!("unknown environment '{n}'"))
        })
        .transpose()?;

    let lock_file = workspace
        .load_lock_file()
        .await
        .ok()
        .map(|r| r.into_lock_file_or_empty_with_warning());

    let env_task_map: HashMap<Environment, HashSet<TaskName>> = if let Some(explicit_environment) =
        explicit_environment
    {
        HashMap::from([(
            explicit_environment.clone(),
            explicit_environment.get_filtered_tasks(),
        )])
    } else {
        workspace
            .environments()
            .iter()
            .filter_map(|env| {
                if verify_current_platform_can_run_environment(env, lock_file.as_ref()).is_ok() {
                    Some((env.clone(), env.get_filtered_tasks()))
                } else {
                    None
                }
            })
            .collect()
    };

    Ok(env_task_map
        .into_iter()
        .map(|(env, task_names)| {
            let env_name = env.name().clone();
            let best_platform = env.best_platform();
            let task_map = task_names
                .into_iter()
                .flat_map(|task_name| {
                    env.task(&task_name, best_platform)
                        .ok()
                        .map(|task| (task_name, task.clone()))
                })
                .collect();
            (env_name, task_map)
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
) -> miette::Result<()> {
    let pixi_platform = lookup_platform(workspace.workspace(), platform.as_ref())?.cloned();
    workspace
        .manifest()
        .add_task(name.clone(), task.clone(), pixi_platform.as_ref(), &feature)?;
    workspace.save().await.into_diagnostic()?;

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
    platform: Option<PixiPlatformName>,
) -> miette::Result<()> {
    let pixi_platform = lookup_platform(workspace.workspace(), platform.as_ref())?.cloned();
    workspace.manifest().add_task(
        name.clone(),
        task.clone(),
        pixi_platform.as_ref(),
        &FeatureName::DEFAULT,
    )?;
    workspace.save().await.into_diagnostic()?;

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
) -> miette::Result<()> {
    let mut to_remove = Vec::new();

    let pixi_platform = lookup_platform(workspace.workspace(), platform.as_ref())?.cloned();

    for name in names.iter() {
        if let Some(pixi_platform) = pixi_platform.as_ref() {
            if !workspace
                .workspace()
                .workspace
                .value
                .tasks(Some(pixi_platform), &feature)?
                .contains_key(name)
            {
                interface
                    .error(&format!(
                        "Task '{}' does not exist on {}",
                        name.fancy_display().bold(),
                        console::style(pixi_platform.name().as_str()).bold(),
                    ))
                    .await;
                continue;
            }
        } else if !workspace
            .workspace()
            .workspace
            .value
            .tasks(None, &feature)?
            .contains_key(name)
        {
            interface
                .error(&format!(
                    "Task `{}` does not exist for the `{}` feature",
                    name.fancy_display().bold(),
                    console::style(&feature).bold(),
                ))
                .await;
            continue;
        }

        // Safe to remove
        to_remove.push(name);
    }

    let mut removed = Vec::with_capacity(to_remove.len());
    for name in to_remove {
        workspace
            .manifest()
            .remove_task(name.clone(), pixi_platform.as_ref(), &feature)?;
        removed.push(name);
    }

    workspace.save().await.into_diagnostic()?;

    for name in removed {
        interface
            .success(&format!("Removed task `{}` ", name.fancy_display().bold()))
            .await;
    }

    Ok(())
}
