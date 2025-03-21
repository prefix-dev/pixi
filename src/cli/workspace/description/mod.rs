pub mod get;
pub mod set;

use crate::{cli::cli_config::WorkspaceConfig, WorkspaceLocator};
use clap::Parser;

/// Commands to manage workspace description.
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
    /// Get the workspace description.
    ///
    /// Example:
    /// `pixi workspace description get`
    Get,
    /// Set the workspace description.
    ///
    /// Example:
    /// `pixi workspace description set "My awesome workspace"`
    Set(set::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .with_ignore_pixi_version_check(true)
        .locate()?;

    match args.command {
        Command::Get => get::execute(workspace).await?,
        Command::Set(args) => set::execute(workspace, args).await?,
    }

    Ok(())
}
