pub mod bump;
pub mod get;
pub mod set;

use crate::Project;
use clap::Parser;
use std::path::PathBuf;

/// Commands to manage project description.
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to 'pixi.toml'
    #[clap(long, global = true)]
    pub manifest_path: Option<PathBuf>,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    Get(get::Args),
    Set(set::Args),
    Major(bump::Args),
    Minor(bump::Args),
    Patch(bump::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    match args.command {
        Command::Get(args) => get::execute(project, args).await?,
        Command::Set(args) => set::execute(project, args).await?,
        Command::Major(args) => bump::execute(bump::BumpType::Major, project, args).await?,
        Command::Minor(args) => bump::execute(bump::BumpType::Minor, project, args).await?,
        Command::Patch(args) => bump::execute(bump::BumpType::Patch, project, args).await?,
    }

    Ok(())
}
