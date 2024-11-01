pub mod get;
pub mod set;

use crate::cli::cli_config::ProjectConfig;
use crate::Project;
use clap::Parser;

/// Commands to manage project description.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: ProjectConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Get the project description.
    Get,
    /// Set the project description.
    Set(set::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(
        args.project_config.manifest_path.as_deref(),
        args.project_config.name,
    )?;

    match args.command {
        Command::Get => get::execute(project).await?,
        Command::Set(args) => set::execute(project, args).await?,
    }

    Ok(())
}
