use std::path::PathBuf;

use clap::Parser;
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
}

/// Runs allows you to manage all the running instances of the project.
/// Note that only the daemon are managed (the tasks executed in the background with the `--detach` or `-d` flag).
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

pub async fn execute(cmd: Args) -> miette::Result<()> {
    match cmd.command {
        Command::List(args) => list::execute(args).await?,
        Command::Kill(args) => kill::execute(args).await?,
        Command::Logs(args) => logs::execute(args).await?,
    };
    Ok(())
}
