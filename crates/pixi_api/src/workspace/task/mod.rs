use std::collections::{HashMap, HashSet};

use fancy_display::FancyDisplay;
use miette::IntoDiagnostic;
use pixi_core::{
    Workspace,
    workspace::{Environment, virtual_packages::verify_current_platform_can_run_environment},
};
use pixi_manifest::{EnvironmentName, FeatureName, Task, TaskName};
use rattler_conda_types::Platform;

use crate::interface::Interface;

pub async fn list_tasks<I: Interface>(
    _interface: &I,
    workspace: Workspace,
    environment: Option<EnvironmentName>,
) -> miette::Result<HashMap<EnvironmentName, HashMap<TaskName, Task>>> {
    let explicit_environment = environment
        .map(|n| {
            workspace
                .environment(&n)
                .ok_or_else(|| miette::miette!("unknown environment '{n}'"))
        })
        .transpose()?;

    let lockfile = workspace.load_lock_file().await.ok();

    let env_task_map: HashMap<Environment, HashSet<TaskName>> =
        if let Some(explicit_environment) = explicit_environment {
            HashMap::from([(
                explicit_environment.clone(),
                explicit_environment.get_filtered_tasks(),
            )])
        } else {
            workspace
                .environments()
                .iter()
                .filter_map(|env| {
                    if verify_current_platform_can_run_environment(env, lockfile.as_ref()).is_ok() {
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
                    env.task(&task_name, Some(best_platform))
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
    workspace: Workspace,
    name: TaskName,
    task: Task,
    feature: FeatureName,
    platform: Option<Platform>,
) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    workspace
        .manifest()
        .add_task(name.clone(), task.clone(), platform, &feature)?;
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
    workspace: Workspace,
    name: TaskName,
    task: Task,
    platform: Option<Platform>,
) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    workspace
        .manifest()
        .add_task(name.clone(), task.clone(), platform, &FeatureName::DEFAULT)?;
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
    workspace: Workspace,
    names: Vec<TaskName>,
    platform: Option<Platform>,
    feature: FeatureName,
) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;
    let mut to_remove = Vec::new();

    for name in names.iter() {
        if let Some(platform) = platform {
            if !workspace
                .workspace()
                .workspace
                .value
                .tasks(Some(platform), &feature)?
                .contains_key(name)
            {
                interface
                    .error(&format!(
                        "Task '{}' does not exist on {}",
                        name.fancy_display().bold(),
                        console::style(platform.as_str()).bold(),
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
        to_remove.push((name, platform));
    }

    let mut removed = Vec::with_capacity(to_remove.len());
    for (name, platform) in to_remove {
        workspace
            .manifest()
            .remove_task(name.clone(), platform, &feature)?;
        removed.push(name);
    }

    workspace.save().await.into_diagnostic()?;

    for name in removed {
        eprintln!(
            "{}Removed task `{}` ",
            console::style(console::Emoji("✔ ", "+")).green(),
            name.fancy_display().bold(),
        );
    }

    Ok(())
}
