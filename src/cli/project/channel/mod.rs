pub mod add;
pub mod list;
pub mod remove;

use crate::Project;
use clap::Parser;
use std::path::PathBuf;

/// Commands to manage project channels.
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
    /// Adds a channel to the project file and updates the lockfile.
    Add(add::Args),
    /// List the channels in the project file.
    List(list::Args),
    /// Remove a channel(s) to the project file and updates the lockfile.
    Remove(remove::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    match args.command {
        Command::Add(args) => add::execute(project, args).await,
        Command::List(args) => list::execute(project, args).await,
        Command::Remove(args) => remove::execute(project, args).await,
    }
}
