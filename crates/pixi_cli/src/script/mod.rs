use clap::{Parser, Subcommand};

pub mod init;
pub mod run;

/// Manage standalone scripts with inline dependency metadata.
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Add a PEP 723 metadata block to a new or existing script.
    Init(init::Args),

    /// Run a script in its isolated environment.
    Run(run::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.command {
        Command::Init(args) => init::execute(args).await,
        Command::Run(args) => run::execute(args).await,
    }
}
