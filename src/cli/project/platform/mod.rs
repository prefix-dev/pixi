pub mod add;
pub mod list;
pub mod remove;

use crate::{cli::cli_config::WorkspaceConfig, WorkspaceLocator};
use clap::Parser;

/// Commands to manage project platforms.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Adds a platform(s) to the project file and updates the lockfile.
    #[clap(visible_alias = "a")]
    Add(add::Args),
    /// List the platforms in the project file.
    #[clap(visible_alias = "ls")]
    List,
    /// Remove platform(s) from the project file and updates the lockfile.
    #[clap(visible_alias = "rm")]
    Remove(remove::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    match args.command {
        Command::Add(args) => add::execute(workspace, args).await,
        Command::List => list::execute(workspace).await,
        Command::Remove(args) => remove::execute(workspace, args).await,
    }
}
