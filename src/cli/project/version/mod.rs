pub mod get;
pub mod set;

use crate::Project;
use clap::Parser;
use std::path::PathBuf;

/// Commands to manage project description.
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to 'pixi.toml'
    #[clap(long, global = true)]
    pub manifest_path: Option<PathBuf>,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Get the project version.
    Get(get::Args),
    /// Set the project version.
    Set(set::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    match args.command {
        Command::Get(args) => get::execute(project, args).await?,
        Command::Set(args) => set::execute(project, args).await?,
    }

    Ok(())
}
