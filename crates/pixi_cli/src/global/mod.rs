use clap::Parser;
use fancy_display::FancyDisplay;
use miette::{IntoDiagnostic, Report, WrapErr};
use rattler_conda_types::NamedChannelOrUrl;
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

/// The channels an environment ends up with once it is set up: an
/// environment that already exists in the manifest keeps its channels
/// (unless `--force-reinstall` recreates it), a new one gets the
/// `--channel` arguments or the config's default channels. Name inference
/// runs before the manifest is touched and has to solve the build backend
/// against these same channels.
fn eventual_environment_channels(
    project: &pixi_global::Project,
    environment: Option<&EnvironmentName>,
    cli_channels: &[NamedChannelOrUrl],
    force_reinstall: bool,
) -> Vec<NamedChannelOrUrl> {
    if !force_reinstall
        && let Some(environment) = environment.and_then(|name| project.environment(name))
    {
        return environment.channels().into_iter().cloned().collect();
    }
    if cli_channels.is_empty() {
        project.config().default_channels()
    } else {
        cli_channels.to_vec()
    }
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use tempfile::tempdir;

    use super::*;

    /// Create a project in an isolated `PIXI_HOME` so tests never read the
    /// global manifest of the machine they run on.
    async fn isolated_project(temp_dir: &std::path::Path) -> pixi_global::Project {
        let pixi_home_dir = temp_dir.join("pixi-home");
        temp_env::async_with_vars(
            [("PIXI_HOME", Some(pixi_home_dir.to_str().unwrap()))],
            pixi_global::Project::discover_or_create(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_eventual_environment_channels() {
        let temp_dir = tempdir().unwrap();
        let mut project = isolated_project(temp_dir.path()).await;

        let existing = EnvironmentName::from_str("existing").unwrap();
        let missing = EnvironmentName::from_str("missing").unwrap();
        let env_channel = NamedChannelOrUrl::from_str("env-channel").unwrap();
        let cli_channel = NamedChannelOrUrl::from_str("cli-channel").unwrap();
        project
            .manifest
            .add_environment(&existing, Some(vec![env_channel.clone()]))
            .unwrap();

        let defaults = project.config().default_channels();

        // No target environment: --channel arguments or the defaults.
        assert_eq!(
            eventual_environment_channels(&project, None, &[], false),
            defaults
        );
        assert_eq!(
            eventual_environment_channels(
                &project,
                None,
                std::slice::from_ref(&cli_channel),
                false
            ),
            vec![cli_channel.clone()]
        );

        // A named environment that does not exist yet behaves the same.
        assert_eq!(
            eventual_environment_channels(
                &project,
                Some(&missing),
                std::slice::from_ref(&cli_channel),
                false
            ),
            vec![cli_channel.clone()]
        );

        // An existing environment keeps its manifest channels; --channel
        // arguments are not applied to it.
        assert_eq!(
            eventual_environment_channels(
                &project,
                Some(&existing),
                std::slice::from_ref(&cli_channel),
                false
            ),
            vec![env_channel]
        );

        // --force-reinstall recreates the environment, so the manifest
        // channels no longer apply.
        assert_eq!(
            eventual_environment_channels(
                &project,
                Some(&existing),
                std::slice::from_ref(&cli_channel),
                true
            ),
            vec![cli_channel]
        );
    }
}
