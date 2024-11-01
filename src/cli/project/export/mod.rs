pub mod conda_environment;
pub mod conda_explicit_spec;

use crate::cli::cli_config::ProjectConfig;
use crate::Project;
use clap::Parser;

/// Commands to export projects to other formats
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: ProjectConfig,

    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Export project environment to a conda explicit specification file
    #[clap(visible_alias = "ces")]
    CondaExplicitSpec(conda_explicit_spec::Args),
    /// Export project environment to a conda environment.yaml file
    CondaEnvironment(conda_environment::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(
        args.project_config.manifest_path.as_deref(),
        args.project_config.name,
    )?;
    match args.command {
        Command::CondaExplicitSpec(args) => conda_explicit_spec::execute(project, args).await?,
        Command::CondaEnvironment(args) => conda_environment::execute(project, args).await?,
    };
    Ok(())
}
