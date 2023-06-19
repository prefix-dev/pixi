use clap::Parser;
mod add;

#[derive(Debug, Parser)]
pub enum Command {
    #[clap(alias = "a")]
    Add(add::Args),
}

/// Global is the main entry point for the part of pixi that executes on the global(system) level.
///
/// It does not touch your system but in comparison to the normal pixi workflow which focuses on project level actions this will work on your system level.
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

pub async fn execute(cmd: Args) -> anyhow::Result<()> {
    match cmd.command {
        Command::Add(args) => add::execute(args).await?,
    };
    Ok(())
}
