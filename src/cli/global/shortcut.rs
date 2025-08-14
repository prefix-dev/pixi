use crate::cli::global::revert_environment_after_error;
use clap::Parser;
use fancy_display::FancyDisplay;
use miette::Context;
use pixi_config::{Config, ConfigCli};
use pixi_core::global::Project;
use pixi_core::global::{EnvironmentName, StateChanges};
use rattler_conda_types::PackageName;
use std::collections::HashMap;

/// Add a shortcut from an environment to your machine.
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct AddArgs {
    /// The package name to add the shortcuts from.
    #[arg(num_args = 1.., value_name = "PACKAGE")]
    packages: Vec<PackageName>,

    /// The environment from which the shortcut should be added.
    #[clap(short, long)]
    environment: EnvironmentName,

    #[clap(flatten)]
    config: ConfigCli,
}

/// Remove shortcuts from your machine.
#[derive(Parser, Debug)]
pub struct RemoveArgs {
    /// The shortcut that should be removed.
    #[arg(num_args = 1.., value_name = "SHORTCUT")]
    shortcuts: Vec<PackageName>,

    #[clap(flatten)]
    config: ConfigCli,
}

/// Interact with the shortcuts on your machine.
#[derive(Parser, Debug)]
#[clap(group(clap::ArgGroup::new("command")))]
pub enum SubCommand {
    #[clap(name = "add")]
    Add(AddArgs),
    #[clap(name = "remove")]
    Remove(RemoveArgs),
}

/// Add or remove shortcuts from your machine
pub async fn execute(args: SubCommand) -> miette::Result<()> {
    match args {
        SubCommand::Add(args) => add(args).await?,
        SubCommand::Remove(args) => remove(args).await?,
    }
    Ok(())
}

pub async fn add(args: AddArgs) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(
        args: &AddArgs,
        project: &mut Project,
    ) -> Result<StateChanges, miette::Error> {
        let env_name = &args.environment;
        let mut state_changes = StateChanges::new_with_env(env_name.clone());
        for name in &args.packages {
            project.manifest.add_shortcut(env_name, name)?;
        }
        state_changes |= project.sync_environment(env_name, None).await?;
        Ok(state_changes)
    }

    let mut project_modified = project_original.clone();
    match apply_changes(&args, &mut project_modified).await {
        Ok(state_changes) => {
            project_modified.manifest.save().await?;
            state_changes.report();
            Ok(())
        }
        Err(err) => {
            if let Err(revert_err) =
                revert_environment_after_error(&args.environment, &project_original).await
            {
                tracing::warn!("Reverting of the operation failed");
                tracing::info!("Reversion error: {:?}", revert_err);
            }
            Err(err)
        }
    }
}

pub async fn remove(args: RemoveArgs) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(
        shortcuts: Vec<PackageName>,
        env_name: &EnvironmentName,
        project: &mut Project,
    ) -> Result<StateChanges, miette::Error> {
        let mut state_changes = StateChanges::new_with_env(env_name.clone());

        for shortcut in shortcuts {
            project
                .manifest
                .remove_shortcut(&shortcut, env_name)
                .wrap_err_with(|| {
                    format!(
                        "Couldn't remove shortcut name '{}' from {} environment",
                        shortcut.as_normalized(),
                        env_name.fancy_display()
                    )
                })?;
        }

        state_changes |= project.sync_environment(env_name, None).await?;
        project.manifest.save().await?;
        Ok(state_changes)
    }

    let to_remove_shortcuts_map: HashMap<EnvironmentName, Vec<PackageName>> = project_original
        .environments()
        .iter()
        .filter_map(|(env_name, env)| {
            env.shortcuts.as_ref().map(|shortcuts| {
                let to_remove = shortcuts
                    .iter()
                    .filter(|shortcut| args.shortcuts.contains(shortcut))
                    .cloned()
                    .collect::<Vec<_>>();
                (!to_remove.is_empty()).then(|| (env_name.clone(), to_remove))
            })?
        })
        .collect();

    if to_remove_shortcuts_map.is_empty() {
        miette::bail!(
            "No shortcuts found with name(s): {}",
            console::style(
                args.shortcuts
                    .iter()
                    .map(|s| s.as_normalized())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .bold()
            .yellow()
        );
    }

    let mut last_updated_project = project_original;
    for (env_name, shortcuts) in to_remove_shortcuts_map {
        let mut project = last_updated_project.clone();
        match apply_changes(shortcuts, &env_name, &mut project)
            .await
            .wrap_err_with(|| {
                format!(
                    "Couldn't remove shortcuts from {}",
                    env_name.fancy_display()
                )
            }) {
            Ok(state_changes) => {
                state_changes.report();
            }
            Err(err) => {
                if let Err(revert_err) =
                    revert_environment_after_error(&env_name, &last_updated_project).await
                {
                    tracing::warn!("Reverting of the operation failed");
                    tracing::info!("Reversion error: {:?}", revert_err);
                }
                return Err(err);
            }
        }
        last_updated_project = project;
    }
    Ok(())
}
