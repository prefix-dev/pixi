use crate::global::revert_environment_after_error;
use clap::Parser;
use fancy_display::FancyDisplay;
use pixi_config::{Config, ConfigCli};
use pixi_global::common::check_all_exposed;
use pixi_global::project::ExposedType;
use pixi_global::{EnvironmentName, Project};
use pixi_global::{StateChange, StateChanges};

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
    let project_original = pixi_global::Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(
        env_name: &EnvironmentName,
        project: &mut Project,
    ) -> miette::Result<StateChanges> {
        // If the environment isn't up-to-date our executable detection afterwards will not work
        if !project.environment_in_sync(env_name).await? {
            let _ = project.install_environment(env_name).await?;
        }

        // See what executables were installed prior to update
        let env_binaries = project.executables_of_direct_dependencies(env_name).await?;

        // Get the exposed binaries from mapping
        let exposed_mapping_binaries = &project
            .environment(env_name)
            .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?
            .exposed;

        // Check if they were all auto-exposed, or if the user manually exposed a subset of them
        let expose_type = if check_all_exposed(&env_binaries, exposed_mapping_binaries) {
            ExposedType::All
        } else {
            ExposedType::Nothing
        };

        // Reinstall the environment
        let environment_update = project.install_environment(env_name).await?;

        let mut state_changes = StateChanges::default();

        state_changes.insert_change(
            env_name,
            StateChange::UpdatedEnvironment(environment_update),
        );

        // Sync executables exposed names with the manifest
        project.sync_exposed_names(env_name, expose_type).await?;

        state_changes |= project.sync_shortcuts(env_name).await?;

        // Expose or prune executables of the new environment
        state_changes |= project
            .expose_executables_from_environment(env_name)
            .await?;

        // Sync completions
        state_changes |= project.sync_completions(env_name).await?;

        Ok(state_changes)
    }

    // Update all environments if the user did not specify any
    let env_names = match args.environments {
        Some(env_names) => env_names,
        None => {
            // prune old environments and completions
            let state_changes = project_original.prune_old_environments().await?;
            state_changes.report();
            #[cfg(unix)]
            {
                let completions_dir = pixi_global::completions::CompletionsDir::from_env().await?;
                completions_dir.prune_old_completions()?;
            }
            project_original.environments().keys().cloned().collect()
        }
    };

    // Apply changes to each environment, only revert changes if an error occurs
    let mut last_updated_project = project_original;

    for env_name in env_names {
        let mut project = last_updated_project.clone();

        match apply_changes(&env_name, &mut project).await {
            Ok(state_changes) => state_changes.report(),
            Err(err) => {
                revert_environment_after_error(&env_name, &last_updated_project).await?;
                return Err(err);
            }
        }
        last_updated_project = project;
    }
    last_updated_project.manifest.save().await?;
    Ok(())
}
