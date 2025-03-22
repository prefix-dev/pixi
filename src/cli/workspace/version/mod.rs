pub mod bump;
pub mod get;
pub mod set;

use crate::{cli::cli_config::WorkspaceConfig, WorkspaceLocator};
use clap::Parser;
use rattler_conda_types::VersionBumpType;

/// Commands to manage workspace version.
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
    /// Get the workspace version.
    Get(get::Args),
    /// Set the workspace version.
    Set(set::Args),
    /// Bump the workspace version to MAJOR.
    Major,
    /// Bump the workspace version to MINOR.
    Minor,
    /// Bump the workspace version to PATCH.
    Patch,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .with_ignore_pixi_version_check(true)
        .locate()?;

    match args.command {
        Command::Get(args) => get::execute(workspace, args).await?,
        Command::Set(args) => set::execute(workspace, args).await?,
        Command::Major => bump::execute(workspace, VersionBumpType::Major).await?,
        Command::Minor => bump::execute(workspace, VersionBumpType::Minor).await?,
        Command::Patch => bump::execute(workspace, VersionBumpType::Patch).await?,
    }

    Ok(())
}
