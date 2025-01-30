pub mod bump;
pub mod get;
pub mod set;

use crate::{cli::cli_config::WorkspaceConfig, Workspace};
use clap::Parser;
use rattler_conda_types::VersionBumpType;

/// Commands to manage project version.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Get the project version.
    Get(get::Args),
    /// Set the project version.
    Set(set::Args),
    /// Bump the project version to MAJOR.
    Major,
    /// Bump the project version to MINOR.
    Minor,
    /// Bump the project version to PATCH.
    Patch,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Workspace::load_or_else_discover(args.project_config.manifest_path.as_deref())?;

    match args.command {
        Command::Get(args) => get::execute(project, args).await?,
        Command::Set(args) => set::execute(project, args).await?,
        Command::Major => bump::execute(project, VersionBumpType::Major).await?,
        Command::Minor => bump::execute(project, VersionBumpType::Minor).await?,
        Command::Patch => bump::execute(project, VersionBumpType::Patch).await?,
    }

    Ok(())
}
