use crate::cli::cli_config::WorkspaceConfig;
use crate::environment::get_update_lock_file_and_prefix;
use crate::lock_file::UpdateMode;
use crate::{UpdateLockFileOptions, WorkspaceLocator};
use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use pixi_config::ConfigCli;

/// Install an environment, both updating the lockfile and installing the environment.
///
/// This command installs an environment, if the lockfile is not up-to-date it will be updated.
///
/// `pixi install` only installs one environment at a time,
/// if you have multiple environments you can select the right one with the `--environment` flag.
/// If you don't provide an environment, the `default` environment will be installed.
///
/// If you want to install all environments, you can use the `--all` flag.
///
/// Running `pixi install` is not required before running other commands like `pixi run` or `pixi shell`.
/// These commands will automatically install the environment if it is not already installed.
///
/// You can use `pixi clean` to remove the installed environments and start fresh.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: WorkspaceConfig,

    #[clap(flatten)]
    pub lock_file_usage: super::LockFileUsageConfig,

    /// The environment to install
    #[arg(long, short)]
    pub environment: Option<Vec<String>>,

    #[clap(flatten)]
    pub config: ConfigCli,

    /// Install all environments
    #[arg(long, short, conflicts_with = "environment")]
    pub all: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.project_config.workspace_locator_start())
        .locate()?
        .with_cli_config(args.config);

    // Install either:
    //
    // 1. specific environments
    // 2. all environments
    // 3. default environment (if no environments are specified)
    let envs = if let Some(envs) = args.environment {
        envs
    } else if args.all {
        workspace
            .environments()
            .iter()
            .map(|env| env.name().to_string())
            .collect()
    } else {
        vec![workspace.default_environment().name().to_string()]
    };

    let mut installed_envs = Vec::with_capacity(envs.len());
    for env in envs {
        let environment = workspace.environment_from_name_or_env_var(Some(env))?;

        // Update the prefix by installing all packages
        get_update_lock_file_and_prefix(
            &environment,
            UpdateMode::Revalidate,
            UpdateLockFileOptions {
                lock_file_usage: args.lock_file_usage.into(),
                no_install: false,
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
            },
        )
        .await?;

        installed_envs.push(environment.name().clone());
    }

    // Message what's installed
    let detached_envs_message =
        if let Ok(Some(path)) = workspace.config().detached_environments().path() {
            format!(" in '{}'", console::style(path.display()).bold())
        } else {
            "".to_string()
        };

    if installed_envs.len() == 1 {
        eprintln!(
            "{}The {} environment has been installed{}.",
            console::style(console::Emoji("✔ ", "")).green(),
            installed_envs[0].fancy_display(),
            detached_envs_message
        );
    } else {
        eprintln!(
            "{}The following environments have been installed: {}\t{}",
            console::style(console::Emoji("✔ ", "")).green(),
            installed_envs.iter().map(|n| n.fancy_display()).join(", "),
            detached_envs_message
        );
    }

    Ok(())
}
