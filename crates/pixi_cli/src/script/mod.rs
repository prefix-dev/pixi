use clap::{Parser, Subcommand};

pub mod add;
pub mod lock;
pub mod remove;
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

    /// Run a script in its isolated environment.
    Run(run::Args),

    /// Resolve a script environment and write its sidecar lock file.
    Lock(lock::Args),

    /// Remove dependencies from a script.
    Remove(remove::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.command {
        Command::Add(args) => add::execute(args).await,
        Command::Run(args) => run::execute(args).await,
        Command::Lock(args) => lock::execute(args).await,
        Command::Remove(args) => remove::execute(args).await,
    }
}
