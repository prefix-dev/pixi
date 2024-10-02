use crate::cli::global::revert_after_error;
use crate::global;
use crate::global::{EnvironmentName, Project};
use clap::Parser;
use miette::Context;
use pixi_config::{Config, ConfigCli};
use std::str::FromStr;

/// Uninstalls environments from the global environment.
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the packages that are to be removed.
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

    async fn apply_changes(args: Args, project: &mut Project) -> Result<(), miette::Error> {
        for env in args.environment {
            let env = EnvironmentName::from_str(&env)
                .wrap_err_with(|| format!("Could not parse environment name: {}", env))?;
            project.remove_environment(&env).await?;
        }

        // TODO: Remove the environment from the environment directory.
        // project.prune_old_environments().await?;

        project.manifest.save().await?;
        Ok(())
    }

    let mut project = project_original.clone();
    if let Err(err) = apply_changes(args, &mut project).await {
        revert_after_error(&project_original)
            .await
            .wrap_err("Could not uninstall environments. Reverting also failed.")?;
        return Err(err);
    }

    Ok(())
}
