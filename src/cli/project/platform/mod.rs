pub mod add;
pub mod list;
pub mod remove;

use crate::{cli::cli_config::ProjectConfig, Workspace};
use clap::Parser;

/// Commands to manage project platforms.
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
    /// Adds a platform(s) to the project file and updates the lockfile.
    #[clap(visible_alias = "a")]
    Add(add::Args),
    /// List the platforms in the project file.
    #[clap(visible_alias = "ls")]
    List,
    /// Remove platform(s) from the project file and updates the lockfile.
    #[clap(visible_alias = "rm")]
    Remove(remove::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Workspace::load_or_else_discover(args.project_config.manifest_path.as_deref())?;

    match args.command {
        Command::Add(args) => add::execute(project, args).await,
        Command::List => list::execute(project).await,
        Command::Remove(args) => remove::execute(project, args).await,
    }
}
