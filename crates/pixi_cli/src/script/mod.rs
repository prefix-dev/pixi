use clap::{Parser, Subcommand};

pub mod remove;

/// Manage standalone scripts with inline dependency metadata.
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Remove dependencies from a script.
    Remove(remove::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.command {
        Command::Remove(args) => remove::execute(args).await,
    }
}
