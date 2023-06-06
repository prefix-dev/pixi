use clap::{CommandFactory, Parser};
use clap_complete::Shell;

use anyhow::Error;
mod add;
mod init;
mod install;
mod run;

#[derive(Parser, Debug)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

/// Generates a completion script for a shell.
#[derive(Parser, Debug)]
pub struct CompletionCommand {
    /// The shell to generate a completion script for (defaults to 'bash').
    #[arg(short, long)]
    shell: Option<Shell>,
}

#[derive(Parser, Debug)]
enum Command {
    Completion(CompletionCommand),
    Init(init::Args),
    #[clap(alias = "a")]
    Add(add::Args),
    #[clap(alias = "r")]
    Run(run::Args),
    #[clap(alias = "i")]
    Install(install::Args),
}

fn completion(args: CompletionCommand) -> Result<(), Error> {
    clap_complete::generate(
        args.shell.unwrap_or(Shell::Bash),
        &mut Args::command(),
        "pax",
        &mut std::io::stdout(),
    );

    Ok(())
}

pub async fn execute() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.command {
        Command::Completion(cmd) => completion(cmd),
        Command::Init(cmd) => init::execute(cmd).await,
        Command::Add(cmd) => add::execute(cmd).await,
        Command::Run(cmd) => run::execute(cmd).await,
        Command::Install(cmd) => install::execute(cmd).await,
    }
}
