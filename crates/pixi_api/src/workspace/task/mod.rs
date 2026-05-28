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
use crate::workspace::platforms::resolve_platforms;

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
) -> miette::Result<Option<PixiPlatform>> {
    let Some(name) = name else { return Ok(None) };
    let workspace_platforms = workspace.workspace_manifest().workspace.platforms.clone();
    Ok(
        resolve_platforms(&workspace_platforms, std::slice::from_ref(name))?
            .into_iter()
            .next(),
    )
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
    let pixi_platform = resolve_task_platform(workspace.workspace(), platform.as_ref())?;
    // Auto-declare the subdir-platform when the user passed a name pixi
    // hasn't seen yet (matches `pixi add --platform <subdir>`). The
    // mutation is idempotent on already-declared entries.
    if let Some(p) = &pixi_platform {
        workspace
            .manifest()
            .add_platforms(std::slice::from_ref(p).iter(), &FeatureName::DEFAULT)?;
    }
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
    let pixi_platform = resolve_task_platform(workspace.workspace(), platform.as_ref())?;
    if let Some(p) = &pixi_platform {
        workspace
            .manifest()
            .add_platforms(std::slice::from_ref(p).iter(), &FeatureName::DEFAULT)?;
    }
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

    // No auto-declare on removal: if the name doesn't match a declared
    // platform we still try the subdir fallback so users can target a
    // dep that lives under an as-yet-undeclared subdir, but we don't
    // mutate `[workspace].platforms` here. A miss surfaces as "Task '...'
    // does not exist on <name>" below.
    let pixi_platform = resolve_task_platform(workspace.workspace(), platform.as_ref())?;

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
