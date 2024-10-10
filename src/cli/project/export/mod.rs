use std::path::PathBuf;
pub mod conda_environment;
pub mod conda_explicit_spec;

use crate::Project;
use clap::Parser;

/// Commands to export projects to other formats
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to `pixi.toml` or `pyproject.toml`
    #[clap(long, global = true)]
    pub manifest_path: Option<PathBuf>,

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
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    match args.command {
        Command::CondaExplicitSpec(args) => conda_explicit_spec::execute(project, args).await?,
        Command::CondaEnvironment(args) => conda_environment::execute(project, args).await?,
    };
    Ok(())
}
