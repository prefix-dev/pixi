use clap::Parser;

use crate::global::{self, EnvironmentName};

mod add;
mod edit;
mod expose;
mod install;
mod list;
mod remove;
mod sync;
mod uninstall;
mod update;

#[derive(Debug, Parser)]
pub enum Command {
    #[clap(visible_alias = "a")]
    Add(add::Args),
    Edit(edit::Args),
    #[clap(visible_alias = "i")]
    Install(install::Args),
    Uninstall(uninstall::Args),
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
    #[clap(visible_alias = "u")]
    Update(update::Args),
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
        Command::Add(args) => add::execute(args).await?,
        Command::Edit(args) => edit::execute(args).await?,
        Command::Install(args) => install::execute(args).await?,
        Command::Uninstall(args) => uninstall::execute(args).await?,
        Command::Remove(args) => remove::execute(args).await?,
        Command::List(args) => list::execute(args).await?,
        Command::Sync(args) => sync::execute(args).await?,
        Command::Expose(subcommand) => expose::execute(subcommand).await?,
        Command::Update(args) => update::execute(args).await?,
    };
    Ok(())
}

/// Reverts the changes made to the project for a specific environment after an error occurred.
async fn revert_environment_after_error(
    env_name: &EnvironmentName,
    project_to_revert_to: &mut global::Project,
) -> miette::Result<()> {
    if project_to_revert_to.environment(env_name).is_some() {
        // We don't want to report on changes done by the reversion
        let _ = project_to_revert_to.sync_environment(env_name).await?;
    }
    Ok(())
}
