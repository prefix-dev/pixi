use clap::Parser;
use miette::Report;

use pixi_api::global::EnvironmentName;
pub(crate) use pixi_api::global::{eventual_environment_channels, revert_environment_after_error};
use pixi_global::report::EnvReport;

mod add;
mod edit;
mod exclude_newer;
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
    ExcludeNewer(exclude_newer::Args),
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

/// Manage globally-installed packages.
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
        Command::ExcludeNewer(args) => exclude_newer::execute(args).await?,
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
    Uninstall,
    Update,
}

impl std::fmt::Display for EnvironmentAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let verb = match self {
            EnvironmentAction::Sync => "sync",
            EnvironmentAction::Install => "install",
            EnvironmentAction::Uninstall => "uninstall",
            EnvironmentAction::Update => "update",
        };
        write!(f, "{verb}")
    }
}

/// Mark an environment as failed in place, so it keeps its position among the
/// environments around it. Only the header line: the reason lands at the end of
/// the run, with the other failures.
fn report_failed_environment(env_name: &EnvironmentName) {
    pixi_global::report::print(&EnvReport::failed(env_name.as_str()));
}

/// Print why each environment failed, then return a single error counting them.
/// Collecting the reasons at the end of the run keeps them readable together,
/// each attributed to its environment. Returns `Ok(())` if there are no errors.
fn report_failed_environments(
    action: EnvironmentAction,
    errors: Vec<(EnvironmentName, Report)>,
) -> miette::Result<()> {
    if errors.is_empty() {
        return Ok(());
    }

    let count = errors.len();
    pixi_global::report::print_failure_heading(&action.to_string(), count);
    for (env_name, err) in &errors {
        pixi_global::report::print(&EnvReport::reason(env_name.as_str(), format!("{err:?}")));
    }

    let plural = if count == 1 { "" } else { "s" };
    Err(miette::miette!(
        "couldn't {action} {count} environment{plural}"
    ))
}
