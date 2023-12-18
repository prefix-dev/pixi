use clap::Parser;
use std::path::PathBuf;

pub mod channel;
pub mod description;

#[derive(Debug, Parser)]
pub enum Command {
    Channel(channel::Args),
    Description(description::Args),
}

/// Modify the project configuration file through the command line.
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
        Command::Channel(args) => channel::execute(args).await?,
        Command::Description(args) => description::execute(args).await?,
    };
    Ok(())
}
