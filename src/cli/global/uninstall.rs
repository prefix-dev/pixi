use crate::cli::global::revert_after_error;
use crate::global;
use crate::global::{EnvironmentName, Project};
use clap::Parser;
use miette::Context;
use pixi_config::{Config, ConfigCli};
use std::str::FromStr;

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
    environment: Vec<String>,

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

    async fn apply_changes(args: Args, project: &mut Project) -> miette::Result<bool> {
        let mut removed = Vec::new();
        for env in args.environment {
            let env = EnvironmentName::from_str(&env)
                .wrap_err_with(|| format!("Could not parse environment name: {}", env))?;
            if project.manifest.remove_environment(&env)? {
                removed.push(env);
            };
        }

        // If no environments were removed, we can return early.
        if removed.is_empty() {
            return Ok(false);
        }

        // Cleanup the project after removing the environments.
        project.prune_old_environments().await?;

        project.manifest.save().await?;
        Ok(true)
    }

    let mut project = project_original.clone();
    match apply_changes(args.clone(), &mut project).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(miette::miette!(format!(
            "Environments not found: {:?} in manifest: {}",
            args.environment,
            project.manifest.path.display()
        ))),
        Err(err) => {
            revert_after_error(&project_original)
                .await
                .wrap_err("Could not uninstall environments. Reverting also failed.")?;
            Err(err)
        }
    }
}
