use crate::cli::global::revert_environment_after_error;
use crate::global::common::check_all_exposed;
use crate::global::project::ExposedType;
use crate::global::{self, InstallChanges};
use crate::global::{EnvironmentName, Project};
use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
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
    ) -> miette::Result<InstallChanges> {
        // let mut state_changes = StateChanges::default();

        // See what executables were installed prior to update
        let env_binaries = project.executables(env_name).await?;

        // Get the exposed binaries from mapping
        let exposed_mapping_binaries = project
            .environment(env_name)
            .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?
            .exposed();

        // Check if they were all auto-exposed, or if the user manually exposed a subset of them
        let expose_type = if check_all_exposed(&env_binaries, exposed_mapping_binaries) {
            ExposedType::default()
        } else {
            ExposedType::subset()
        };

        // Reinstall the environment
        let install_changes = project.install_environment(env_name).await?;

        // Sync executables exposed names with the manifest
        project.sync_exposed_names(env_name, expose_type).await?;

        // Expose or prune executables of the new environment
        let _ = project
            .expose_executables_from_environment(env_name)
            .await?;

        // state_changes.insert_change(env_name, global::StateChange::UpdatedEnvironment);

        Ok(install_changes)
    }

    // Update all environments if the user did not specify any
    let env_names = match args.environments {
        Some(env_names) => env_names,
        None => project_original.environments().keys().cloned().collect(),
    };

    // Apply changes to each environment, only revert changes if an error occurs
    let mut last_updated_project = project_original;
    // let mut state_changes = StateChanges::default();
    for env_name in env_names {
        let mut project = last_updated_project.clone();
        let dependencies = project
            .environment(&env_name)
            .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?
            .dependencies()
            .keys()
            .cloned()
            .collect_vec();

        match apply_changes(&env_name, &mut project).await {
            Ok(ic) => ic.report_update_changes(&env_name, dependencies),
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
