use clap::{Parser, Subcommand};

pub mod add;
pub mod init;
pub mod lock;
pub mod run;

/// Manage standalone scripts with inline dependency metadata.
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Add conda or PyPI dependencies to a script.
    Add(add::Args),

    /// Add a PEP 723 metadata block to a new or existing script.
    Init(init::Args),

    /// Run a script in its isolated environment.
    Run(run::Args),

    /// Resolve a script environment and write its sidecar lock file.
    Lock(lock::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.command {
        Command::Add(args) => add::execute(args).await,
        Command::Init(args) => init::execute(args).await,
        Command::Run(args) => run::execute(args).await,
        Command::Lock(args) => lock::execute(args).await,
    }
}
