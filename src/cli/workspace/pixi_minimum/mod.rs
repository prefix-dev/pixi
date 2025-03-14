pub mod get;
pub mod set;
pub mod unset;
pub mod verify;

use crate::cli::cli_config::WorkspaceConfig;
use crate::WorkspaceLocator;
use clap::Parser;

/// Commands to manage workspace name.
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
    /// Get the pixi minimum version requirement.
    Get,
    /// Set the pixi minimum version requirement.
    ///
    /// Example:
    /// `pixi workspace pixi-minimum set 0.42`
    Set(set::Args),
    /// Remove the pixi minimum version requirement.
    Unset,
    /// Verify the pixi minimum version requirement.
    Verify,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    match args.command {
        Command::Get => get::execute(workspace).await?,
        Command::Set(args) => set::execute(workspace, args).await?,
        Command::Unset => unset::execute(workspace).await?,
        Command::Verify => verify::execute(workspace).await?,
    }

    Ok(())
}
