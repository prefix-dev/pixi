use clap::Parser;

use crate::global;

mod expose;
mod install;
mod list;
mod remove;
mod sync;

#[derive(Debug, Parser)]
pub enum Command {
    #[clap(visible_alias = "i")]
    Install(install::Args),
    // TODO: Needs to adapted
    #[clap(visible_alias = "rm")]
    Remove(remove::Args),
    // TODO: Needs to adapted
    #[clap(visible_alias = "ls")]
    List(list::Args),
    #[clap(visible_alias = "s")]
    Sync(sync::Args),
    #[clap(visible_alias = "e")]
    #[command(subcommand)]
    Expose(expose::SubCommand),
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
        Command::Sync(args) => sync::execute(args).await?,
        Command::Expose(subcommand) => expose::execute(subcommand).await?,
    };
    Ok(())
}

/// Reverts the changes made to the project after an error occurred.
async fn revert_after_error(project_original: &global::Project) -> miette::Result<()> {
    project_original.sync().await?;
    Ok(())
}
