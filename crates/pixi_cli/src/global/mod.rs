use clap::Parser;
use fancy_display::FancyDisplay;
use miette::{IntoDiagnostic, Report, WrapErr};
use tokio::fs as tokio_fs;

use pixi_global::EnvironmentName;

mod add;
mod edit;
mod expose;
mod global_specs;
pub mod install;
mod list;
mod remove;
mod shortcut;
mod sync;
mod tree;
mod uninstall;
mod update;
mod upgrade;
mod upgrade_all;

#[derive(Debug, Parser)]
pub enum Command {
    #[clap(visible_alias = "a")]
    Add(add::Args),
    Edit(edit::Args),
    #[clap(visible_alias = "i")]
    Install(install::Args),
    Uninstall(uninstall::Args),
    #[clap(visible_alias = "rm")]
    Remove(remove::Args),
    #[clap(visible_alias = "ls")]
    List(list::Args),
    #[clap(visible_alias = "s")]
    Sync(sync::Args),
    #[clap(visible_alias = "e")]
    #[command(subcommand)]
    Expose(expose::SubCommand),
    #[command(subcommand)]
    Shortcut(shortcut::SubCommand),
    Update(update::Args),
    #[command(hide = true)]
    Upgrade(upgrade::Args),
    #[clap(alias = "ua")]
    #[command(hide = true)]
    UpgradeAll(upgrade_all::Args),
    #[clap(visible_alias = "t")]
    Tree(tree::Args),
}

/// Subcommand for global package management actions.
///
/// Install packages on the user level.
/// Into to the `$PIXI_HOME` directory, which defaults to `~/.pixi`.
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

/// Maps global command enum variants to their function handlers.
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
        Command::Shortcut(subcommand) => shortcut::execute(subcommand).await?,
        Command::Update(args) => update::execute(args).await?,
        Command::Upgrade(args) => upgrade::execute(args).await?,
        Command::UpgradeAll(args) => upgrade_all::execute(args).await?,
        Command::Tree(args) => tree::execute(args).await?,
    };
    Ok(())
}

/// The operation that failed for one or more environments; determines the verb
/// used in the resulting error messages.
#[derive(Debug, Clone, Copy)]
enum EnvironmentAction {
    Sync,
    Install,
    Remove,
}

impl std::fmt::Display for EnvironmentAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let verb = match self {
            EnvironmentAction::Sync => "sync",
            EnvironmentAction::Install => "install",
            EnvironmentAction::Remove => "remove",
        };
        write!(f, "{verb}")
    }
}

/// Warns about each failed environment with its full error, then returns a
/// single error naming every environment the operation failed for. Returns
/// `Ok(())` if there are no errors.
fn report_failed_environments(
    action: EnvironmentAction,
    errors: Vec<(EnvironmentName, Report)>,
) -> miette::Result<()> {
    if errors.is_empty() {
        return Ok(());
    }
    for (env_name, err) in &errors {
        tracing::warn!(
            "Couldn't {action} environment {}\n{err:?}",
            env_name.fancy_display()
        );
    }
    let failed_envs = errors
        .iter()
        .map(|(env_name, _)| env_name.fancy_display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(miette::miette!(
        "Couldn't {action} the following environments: {failed_envs}"
    ))
}

/// Reverts the changes made to the project for a specific environment after an error occurred.
async fn revert_environment_after_error(
    env_name: &EnvironmentName,
    project_to_revert_to: &pixi_global::Project,
) -> miette::Result<()> {
    if project_to_revert_to.environment(env_name).is_some() {
        // We don't want to report on changes done by the reversion
        let _ = project_to_revert_to
            .sync_environment(env_name, None)
            .await
            .wrap_err_with(|| format!("Couldn't revert environment {env_name}"))?;
    } else {
        // clean up if directory exists for the failed new environment
        let env_dir_path = project_to_revert_to.env_root_path().join(env_name.as_str());
        if env_dir_path.exists() {
            tokio_fs::remove_dir_all(&env_dir_path)
                .await
                .into_diagnostic()?;
            tracing::debug!(
                "Cleaned up failed environment directory: {}",
                env_dir_path.display()
            );
        }
    }
    Ok(())
}
