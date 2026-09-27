use std::process::ExitCode;

pub mod get;
pub mod set;
pub mod unset;

use clap::Parser;
use pixi_core::WorkspaceLocator;

use crate::cli_config::WorkspaceConfig;

/// Commands to manage the Python version requirement (requires-python) in pyproject.toml.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Get the requires-python version requirement.
    Get,
    /// Set the requires-python version requirement.
    ///
    /// Example:
    /// `pixi workspace requires-python set ">=3.10"`
    Set(set::Args),
    /// Remove the requires-python version requirement.
    Unset,
}

pub async fn execute(args: Args) -> miette::Result<ExitCode> {
    let workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    match args.command {
        Command::Get => get::execute(workspace).await?,
        Command::Set(args) => set::execute(workspace, args).await?,
        Command::Unset => unset::execute(workspace).await?,
    }

    Ok(ExitCode::SUCCESS)
}
