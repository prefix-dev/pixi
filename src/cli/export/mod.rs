use clap::Parser;

mod conda;

#[derive(Debug, Parser)]
pub enum Command {
    #[clap(alias = "c")]
    Conda(conda::Args),
}

/// Subcommand for exporting dependencies to additional formats
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

pub async fn execute(cmd: Args) -> miette::Result<()> {
    match cmd.command {
        Command::Conda(args) => conda::execute(args).await?,
    };
    Ok(())
}
