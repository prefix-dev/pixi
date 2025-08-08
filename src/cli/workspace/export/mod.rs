pub mod conda_environment;
pub mod conda_explicit_spec;
pub mod split_lockfile;

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
    /// Split workspace lockfile for each non-empty environment and platform
    SplitLockfile(split_lockfile::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.command {
        Command::CondaExplicitSpec(args) => conda_explicit_spec::execute(args).await?,
        Command::CondaEnvironment(args) => conda_environment::execute(args).await?,
        Command::SplitLockfile(args) => split_lockfile::execute(args).await?,
    };
    Ok(())
}
