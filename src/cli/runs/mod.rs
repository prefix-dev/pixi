use std::path::PathBuf;

use clap::Parser;

use crate::Project;
mod clear;
mod clear_all;
mod kill;
mod list;
mod logs;

#[derive(Debug, Parser)]
pub enum Command {
    #[clap(alias = "ls")]
    List(list::Args),
    #[clap()]
    Kill(kill::Args),
    #[clap()]
    Logs(logs::Args),
    #[clap()]
    Clear(clear::Args),
    ClearAll(clear_all::Args),
}

/// Runs allows you to manage all the running instances of the project.
/// Note that only the daemon are managed (the tasks executed in the background with the `--detach` or `-d` flag).
#[derive(Debug, Parser)]
pub struct Args {
    /// The path to 'pixi.toml'
    #[clap(long, global = true)]
    pub manifest_path: Option<PathBuf>,

    /// The subcommand to execute
    #[command(subcommand)]
    command: Command,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    match args.command {
        Command::List(args) => list::execute(project, args).await?,
        Command::Kill(args) => kill::execute(project, args).await?,
        Command::Logs(args) => logs::execute(project, args).await?,
        Command::Clear(args) => clear::execute(project, args).await?,
        Command::ClearAll(args) => clear_all::execute(project, args).await?,
    };
    Ok(())
}
