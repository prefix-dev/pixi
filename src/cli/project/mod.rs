use crate::cli::cli_config::ProjectConfig;
use clap::Parser;

pub mod channel;
pub mod description;
pub mod environment;
pub mod platform;
pub mod version;

#[derive(Debug, Parser)]
pub enum Command {
    Channel(channel::Args),
    Description(description::Args),
    Platform(platform::Args),
    Version(version::Args),
    Environment(environment::Args),
}

/// Modify the project configuration file through the command line.
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Command,

    #[clap(flatten)]
    pub project_config: ProjectConfig,
}

pub async fn execute(cmd: Args) -> miette::Result<()> {
    match cmd.command {
        Command::Channel(args) => channel::execute(args).await?,
        Command::Description(args) => description::execute(args).await?,
        Command::Platform(args) => platform::execute(args).await?,
        Command::Version(args) => version::execute(args).await?,
        Command::Environment(args) => environment::execute(args).await?,
    };
    Ok(())
}
