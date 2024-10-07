use crate::cli::global::revert_environment_after_error;
use crate::global::{self, StateChanges};
use crate::global::{EnvironmentName, Project};
use clap::Parser;
use miette::Context;
use pixi_config::{Config, ConfigCli};

/// Uninstalls environments from the global environment.
///
/// Example:
/// # Uninstall one environment
/// pixi global uninstall pixi-pack
/// # Uninstall multiple environments
/// pixi global uninstall pixi-pack rattler-build
#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the environments that are to be removed.
    #[arg(num_args = 1..)]
    environment: Vec<EnvironmentName>,

    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,

    #[clap(flatten)]
    config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = global::Project::discover_or_create(args.assume_yes)
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(
        env_name: &EnvironmentName,
        project_modified: &mut Project,
        state_changes: &mut StateChanges,
    ) -> miette::Result<()> {
        state_changes.push_change(project_modified.manifest.remove_environment(env_name)?);

        // Cleanup the project after removing the environments.
        *state_changes |= project_modified.prune_old_environments().await?;

        project_modified.manifest.save().await?;

        Ok(())
    }

    let mut project_modified = project_original.clone();
    let mut state_changes = StateChanges::default();
    for env_name in &args.environment {
        if let Err(err) = apply_changes(env_name, &mut project_modified, &mut state_changes).await {
            state_changes.report();
            revert_environment_after_error(env_name, &project_original)
                .await
                .wrap_err_with(|| {
                    format!("Could not uninstall environment '{env_name}'. Reverting also failed.")
                })?;
            return Err(err);
        }
    }
    state_changes.report();
    Ok(())
}
