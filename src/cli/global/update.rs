use crate::cli::global::revert_environment_after_error;
use crate::global::{self, StateChanges};
use crate::global::{EnvironmentName, Project};
use clap::Parser;
use fancy_display::FancyDisplay;
use pixi_config::{Config, ConfigCli};

/// Updates environments in the global environment.
#[derive(Parser, Debug, Clone)]
pub struct Args {
    /// Specifies the environments that are to be updated.
    environments: Option<Vec<EnvironmentName>>,

    #[clap(flatten)]
    config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = global::Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(
        env_name: &EnvironmentName,
        project: &mut Project,
    ) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::default();
        // Reinstall the environment
        project.install_environment(env_name).await?;

        // Remove broken executables
        state_changes |= project.remove_broken_expose_names(env_name).await?;

        eprintln!(
            "{}Updated environment: {}.",
            console::style(console::Emoji("✔ ", "")).green(),
            env_name.fancy_display()
        );

        Ok(state_changes)
    }

    // Update all environments if the user did not specify any
    let env_names = match args.environments {
        Some(env_names) => env_names,
        None => project_original.environments().keys().cloned().collect(),
    };

    // Apply changes to each environment, only revert changes if an error occurs
    let mut last_updated_project = project_original;
    let mut state_changes = StateChanges::default();
    for env_name in env_names {
        let mut project = last_updated_project.clone();
        match apply_changes(&env_name, &mut project).await {
            Ok(sc) => state_changes |= sc,
            Err(err) => {
                revert_environment_after_error(&env_name, &mut last_updated_project).await?;
                return Err(err);
            }
        }
        last_updated_project = project;
    }
    last_updated_project.manifest.save().await?;
    state_changes.report();
    Ok(())
}
