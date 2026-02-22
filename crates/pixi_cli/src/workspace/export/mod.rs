pub mod conda_environment;
pub mod conda_explicit_spec;

use clap::Parser;

/// Commands to export workspaces to other formats
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Export workspace environment to a conda explicit specification file
    #[clap(visible_alias = "ces")]
    CondaExplicitSpec(conda_explicit_spec::Args),
    /// Export workspace environment to a conda environment.yaml file
    CondaEnvironment(conda_environment::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.command {
        Command::CondaExplicitSpec(args) => conda_explicit_spec::execute(args).await?,
        Command::CondaEnvironment(args) => conda_environment::execute(args).await?,
    };
    Ok(())
}
