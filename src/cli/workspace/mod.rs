use crate::cli::cli_config::WorkspaceConfig;
use clap::Parser;

pub mod channel;
pub mod description;
pub mod environment;
pub mod export;
pub mod name;
pub mod pixi_minimum;
pub mod platform;
pub mod system_requirements;
pub mod version;

#[derive(Debug, Parser)]
pub enum Command {
    Channel(channel::Args),
    Description(description::Args),
    Platform(platform::Args),
    Version(version::Args),
    Environment(environment::Args),
    Export(export::Args),
    Name(name::Args),
    SystemRequirements(system_requirements::Args),
    PixiMinimum(pixi_minimum::Args),
}

/// Modify the workspace configuration file through the command line.
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Command,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,
}

pub async fn execute(cmd: Args) -> miette::Result<()> {
    match cmd.command {
        Command::Channel(args) => channel::execute(args).await?,
        Command::Description(args) => description::execute(args).await?,
        Command::Platform(args) => platform::execute(args).await?,
        Command::Version(args) => version::execute(args).await?,
        Command::Environment(args) => environment::execute(args).await?,
        Command::Export(cmd) => export::execute(cmd).await?,
        Command::Name(args) => name::execute(args).await?,
        Command::SystemRequirements(args) => system_requirements::execute(args).await?,
        Command::PixiMinimum(args) => pixi_minimum::execute(args).await?,
    };
    Ok(())
}
