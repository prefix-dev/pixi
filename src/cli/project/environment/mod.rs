pub mod add;
pub mod list;
pub mod remove;

use crate::{cli::cli_config::WorkspaceConfig, WorkspaceLocator};
use clap::Parser;

/// Commands to manage project environments.
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
    /// Adds an environment to the manifest file.
    #[clap(visible_alias = "a")]
    Add(add::Args),
    /// List the environments in the manifest file.
    #[clap(visible_alias = "ls")]
    List,
    /// Remove an environment from the manifest file.
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
