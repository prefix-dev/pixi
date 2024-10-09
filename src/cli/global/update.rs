use crate::cli::global::revert_environment_after_error;
use crate::global::{self, StateChanges};
use crate::global::{EnvironmentName, Project};
use clap::Parser;
use itertools::Itertools;
use pixi_config::{Config, ConfigCli};
use pixi_utils::executable_from_path;

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

        // Remove the outdated exposed binaries from the manifest
        if let Some(environment) = project.environment(env_name) {
            // Find all installed records and executables
            let prefix = project.environment_prefix(env_name).await?;
            let prefix_records = &prefix.find_installed_packages(None).await?;
            let all_executables = &prefix.find_executables(prefix_records.as_slice());

            // Find the exposed names that are no longer installed in the environment
            let to_remove = environment
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

            // Removed the removable exposed names from the manifest
            for exposed_name in &to_remove {
                project
                    .manifest
                    .remove_exposed_name(env_name, exposed_name)?;
            }
        }

        // Remove outdated binaries
        state_changes |= project.prune_exposed(env_name).await?;
        eprintln!(
            "{}Updated environment: '{}'.",
            console::style(console::Emoji("✔ ", "")).green(),
            env_name
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
