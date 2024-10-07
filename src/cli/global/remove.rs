use crate::cli::global::revert_environment_after_error;
use crate::cli::has_specs::HasSpecs;
use crate::global::{EnvironmentName, Project};
use clap::Parser;
use itertools::Itertools;
use miette::Context;
use pixi_config::{Config, ConfigCli};
use rattler_conda_types::MatchSpec;

/// Removes a package previously installed into a globally accessible location via `pixi global install`.
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the packages that are to be removed.
    #[arg(num_args = 1..)]
    packages: Vec<String>,

    /// Specifies the environment that the dependencies need to be added to.
    #[clap(short, long, required = true)]
    environment: EnvironmentName,

    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,

    #[clap(flatten)]
    config: ConfigCli,
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.packages.iter().map(AsRef::as_ref).collect()
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = Project::discover_or_create(args.assume_yes)
        .await?
        .with_cli_config(config.clone());

    if project_original.environment(&args.environment).is_none() {
        miette::bail!("Environment {} doesn't exist. You can create a new environment with `pixi global install`.", &args.environment);
    }

    async fn apply_changes(
        env_name: &EnvironmentName,
        specs: &[MatchSpec],
        project: &mut Project,
    ) -> miette::Result<()> {
        // Add specs to the manifest
        for spec in specs {
            project.manifest.remove_dependency(env_name, spec)?;
        }

        // Sync environment
        project.sync_environment(env_name).await?;

        project.manifest.save().await?;
        Ok(())
    }

    let mut project = project_original.clone();
    let specs = args
        .specs()?
        .into_iter()
        .map(|(_, specs)| specs)
        .collect_vec();

    if let Err(err) = apply_changes(&args.environment, specs.as_slice(), &mut project).await {
        revert_environment_after_error(&args.environment, &project_original)
            .await
            .wrap_err(format!(
                "Could not add {:?}. Reverting also failed.",
                args.packages
            ))?;
        return Err(err);
    }

    Ok(())
}
