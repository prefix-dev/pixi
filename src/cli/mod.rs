use clap::{CommandFactory, Parser};
use clap_complete::Shell;

use crate::environment::get_up_to_date_prefix;
use crate::Project;
use anyhow::Error;

mod add;
mod init;
mod install;
mod run;

#[derive(Parser, Debug)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
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
        "pixi",
        &mut std::io::stdout(),
    );

    Ok(())
}

/// Run the project initialization when there is a manifest available.
/// This is run when only running `pixi`, which aligns with yarns implementation.
async fn default() -> Result<(), Error> {
    let project = Project::discover()?;
    get_up_to_date_prefix(&project).await?;
    // Emit success
    eprintln!(
        "{}Project in {} is ready to use!",
        console::style(console::Emoji("✔ ", "")).green(),
        project.root().display()
    );
    Ok(())
}

pub async fn execute() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.command {
        Some(Command::Completion(cmd)) => completion(cmd),
        Some(Command::Init(cmd)) => init::execute(cmd).await,
        Some(Command::Add(cmd)) => add::execute(cmd).await,
        Some(Command::Run(cmd)) => run::execute(cmd).await,
        Some(Command::Install(cmd)) => install::execute(cmd).await,
        None => default().await,
    }
}
