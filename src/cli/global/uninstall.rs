use crate::cli::global::revert_environment_after_error;
use crate::global;
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
    ) -> miette::Result<bool> {
        if !project.manifest.remove_environment(env_name)? {
            return Ok(false);
        };

        // Cleanup the project after removing the environments.
        project.prune_old_environments().await?;

        project.manifest.save().await?;
        Ok(true)
    }

    let mut project = project_original.clone();
    for env_name in &args.environment {
        match apply_changes(env_name, &mut project).await {
            Ok(true) => (),
            Ok(false) => {
                miette::bail!(
                    "Environment '{env_name}' not found in manifest '{}'",
                    project.manifest.path.display()
                );
            }
            Err(err) => {
                if project_original.environment(env_name).is_some() {
                    revert_environment_after_error(env_name, &project_original)
                        .await
                        .wrap_err_with(|| {
                            format!(
                            "Could not uninstall environment '{env_name}'. Reverting also failed."
                        )
                        })?;
                    return Err(err);
                }
            }
        }
    }
    Ok(())
}
