use clap::Parser;

mod install;
mod list;
mod remove;
mod sync;
mod upgrade;
mod upgrade_all;

#[derive(Debug, Parser)]
pub enum Command {
    // TODO: Needs to adapted
    #[clap(visible_alias = "i")]
    Install(install::Args),
    // TODO: Needs to adapted
    #[clap(visible_alias = "rm")]
    Remove(remove::Args),
    // TODO: Needs to adapted
    #[clap(visible_alias = "ls")]
    List(list::Args),
    // TODO: Needs to removed
    #[clap(visible_alias = "u")]
    Upgrade(upgrade::Args),
    // TODO: Needs to removed
    #[clap(visible_alias = "ua")]
    UpgradeAll(upgrade_all::Args),
    #[clap(visible_alias = "s")]
    Sync(sync::Args),
}

/// Subcommand for global package management actions
///
/// Install packages on the user level.
/// Example:
///    pixi global install my_package
///    pixi global remove my_package
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
        Command::Sync(args) => sync::execute(args).await?,
    };
    Ok(())
}
