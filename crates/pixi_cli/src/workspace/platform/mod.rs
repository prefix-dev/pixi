pub mod add;
pub mod list;
pub mod remove;

use clap::Parser;
use pixi_core::WorkspaceLocator;

use crate::cli_config::WorkspaceConfig;

/// Commands to manage workspace platforms.
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
    /// Adds a platform(s) to the workspace file and updates the lockfile.
    #[clap(visible_alias = "a")]
    Add(add::Args),
    /// List the platforms in the workspace file.
    #[clap(visible_alias = "ls")]
    List,
    /// Remove platform(s) from the workspace file and updates the lockfile.
    #[clap(visible_alias = "rm")]
    Remove(remove::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        // Avoid throwing warning messages as we're modifying the workspace
        .with_emit_warnings(
            !matches!(args.command, Command::Add(_)) && !matches!(args.command, Command::Remove(_)),
        )
        .locate()?;

    match args.command {
        Command::Add(args) => add::execute(workspace, args).await,
        Command::List => list::execute(workspace).await,
        Command::Remove(args) => remove::execute(workspace, args).await,
    }
}
