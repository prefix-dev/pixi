use fancy_display::FancyDisplay;
use itertools::Itertools;
use pixi_core::{
    InstallFilter, UpdateLockFileOptions, Workspace,
    environment::{LockFileUsage, get_update_lock_file_and_prefix},
    lock_file::{ReinstallEnvironment, UpdateMode},
};

use crate::interface::Interface;

mod options;

pub use options::ReinstallOptions;

pub async fn reinstall<I: Interface>(
    interface: &I,
    workspace: &Workspace,
    options: ReinstallOptions,
    lock_file_usage: LockFileUsage,
) -> miette::Result<()> {
    // Install either:
    //
    // 1. some specific environments
    // 2. all environments
    // 3. default environment (if no environments are specified)
    let envs = if let ReinstallEnvironment::Some(envs) = options.reinstall_environments {
        envs.into_iter().collect()
    } else if options.reinstall_environments == ReinstallEnvironment::All {
        workspace
            .environments()
            .iter()
            .map(|env| env.name().to_string())
            .collect()
    } else {
        vec![workspace.default_environment().name().to_string()]
    };

    let mut installed_envs = Vec::with_capacity(envs.len());
    for env in envs {
        let environment = workspace.environment_from_name_or_env_var(Some(env))?;

        // Update the prefix by installing all packages
        get_update_lock_file_and_prefix(
            &environment,
            UpdateMode::Revalidate,
            UpdateLockFileOptions {
                lock_file_usage,
                no_install: false,
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
            },
            options.reinstall_packages.clone(),
            &InstallFilter::default(),
        )
        .await?;

        installed_envs.push(environment.name().clone());
    }

    // Message what's installed
    let detached_envs_message =
        if let Ok(Some(path)) = workspace.config().detached_environments().path() {
            format!(" in '{}'", console::style(path.display()).bold())
        } else {
            "".to_string()
        };

    let is_cli = interface.is_cli().await;
    let installed_envs_names: Vec<String> = installed_envs
        .iter()
        .map(|env| {
            if is_cli {
                env.fancy_display().to_string()
            } else {
                env.to_string()
            }
        })
        .collect();

    if installed_envs.len() == 1 {
        interface
            .success(&format!(
                "The {} environment has been re-installed{}.",
                installed_envs_names[0], detached_envs_message
            ))
            .await;
    } else {
        interface
            .success(&format!(
                "The following environments have been re-installed: {}\t{}",
                installed_envs_names.iter().join(", "),
                detached_envs_message
            ))
            .await;
    }

    Ok(())
}
