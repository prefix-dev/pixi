pub mod set;

use clap::Parser;
use pixi_core::WorkspaceLocator;

use crate::cli_config::WorkspaceConfig;

/// Commands to manage the Python version requirement.
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
    /// Set the `requires-python` version specifier.
    ///
    /// Example:
    /// `pixi workspace requires-python set ">=3.10"`
    ///
    /// Note: This command is only supported for `pyproject.toml` manifests.
    Set(set::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace_locator = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .with_ignore_pixi_version_check(true);

    match args.command {
        Command::Set(args) => set::execute(workspace_locator.locate()?, args).await?,
    }

    Ok(())
}
