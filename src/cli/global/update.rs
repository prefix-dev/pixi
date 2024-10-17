use crate::cli::global::revert_environment_after_error;
use crate::global::{self, StateChanges};
use crate::global::{EnvironmentName, Project};
use clap::Parser;
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
        let prefix = project.environment_prefix(env_name).await?;

        // Update the environment
        project.install_environment(env_name).await?;

        // Remove broken executables
        state_changes |= project.remove_broken_expose_names(env_name).await?;

        state_changes.insert_change(env_name, global::StateChange::UpdatedEnvironment);

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
                state_changes.report();
                revert_environment_after_error(&env_name, &last_updated_project).await?;
                return Err(err);
            }
        }
        last_updated_project = project;
    }
    last_updated_project.manifest.save().await?;
    state_changes.report();
    Ok(())
}

async fn remove_invalid_exposed_mappings(
    prefix: crate::prefix::Prefix,
    prefix_records: &Vec<rattler_conda_types::PrefixRecord>,
    project: &mut Project,
    env_name: &EnvironmentName,
) -> Result<StateChanges, miette::Error> {
    // Remove exposed executables from the manifest that are not valid anymore
    let all_executables = &prefix.find_executables(prefix_records.as_slice());
    let parsed_environment = project
        .environment(env_name)
        .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?;
    let to_remove = parsed_environment
        .exposed
        .iter()
        .filter_map(|mapping| {
            // If the executable is still requested, do not remove the mapping
            if all_executables
                .iter()
                .any(|(_, path)| executable_from_path(path) == mapping.executable_name())
            {
                tracing::debug!("Not removing mapping to: {}", mapping.executable_name());
                return None;
            }
            // Else do remove the mapping
            Some(mapping.exposed_name().clone())
        })
        .collect_vec();
    for exposed_name in &to_remove {
        project
            .manifest
            .remove_exposed_name(env_name, exposed_name)?;
    }

    // Remove all exposed executables from the file system that are not mentioned in the manifest
    project.prune_exposed(env_name).await
}
