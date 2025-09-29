use std::collections::{HashMap, HashSet};

use pixi_core::{
    Workspace,
    workspace::{Environment, virtual_packages::verify_current_platform_can_run_environment},
};
use pixi_manifest::{EnvironmentName, Task, TaskName};

use crate::interface::Interface;

pub(crate) async fn list_tasks<I: Interface>(
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
