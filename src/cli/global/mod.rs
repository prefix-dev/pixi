use clap::Parser;

mod common;
mod install;
mod list;
mod remove;
mod upgrade;
mod upgrade_all;

#[derive(Debug, Parser)]
pub enum Command {
    // BREAK: This should only have the `i` as an alias
    #[clap(visible_alias = "i", alias = "a")]
    Install(install::Args),
    // BREAK: This should only have the `rm` as an alias
    #[clap(visible_alias = "rm", alias = "r")]
    Remove(remove::Args),
    #[clap(visible_alias = "ls")]
    List(list::Args),
    #[clap(visible_alias = "u")]
    Upgrade(upgrade::Args),
    #[clap(visible_alias = "ua")]
    UpgradeAll(upgrade_all::Args),
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
        Command::Remove(args) => remove::execute(args).await?,
        Command::List(args) => list::execute(args).await?,
        Command::Upgrade(args) => upgrade::execute(args).await?,
        Command::UpgradeAll(args) => upgrade_all::execute(args).await?,
    };
    Ok(())
}
