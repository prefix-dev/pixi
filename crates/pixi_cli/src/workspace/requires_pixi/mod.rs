pub mod get;
pub mod set;
pub mod unset;
pub mod verify;

use clap::Parser;
use pixi_core::WorkspaceLocator;

use crate::cli_config::WorkspaceConfig;

/// Commands to manage the pixi minimum version requirement.
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
    let workspace_locator = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .with_ignore_pixi_version_check(true);

    match args.command {
        Command::Get => get::execute(workspace_locator.locate()?).await?,
        Command::Set(args) => set::execute(workspace_locator.locate()?, args).await?,
        Command::Unset => unset::execute(workspace_locator.locate()?).await?,
        Command::Verify => verify::execute(workspace_locator.locate()?).await?,
    }

    Ok(())
}
