use clap::Parser;
mod install;

#[derive(Debug, Parser)]
pub enum Command {
    #[clap(alias = "a")]
    Install(install::Args),
}

/// Global is the main entry point for the part of pixi that executes on the global(system) level.
///
/// It does not touch your system but in comparison to the normal pixi workflow which focuses on project level actions this will work on your system level.
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

pub async fn execute(cmd: Args) -> miette::Result<()> {
    match cmd.command {
        Command::Install(args) => install::execute(args).await?,
    };
    Ok(())
}
