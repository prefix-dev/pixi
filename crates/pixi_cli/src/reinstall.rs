use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use pixi_config::ConfigCli;
use pixi_core::environment::{InstallFilter, get_update_lock_file_and_prefix};
use pixi_core::lock_file::{ReinstallPackages, UpdateMode};
use pixi_core::{UpdateLockFileOptions, WorkspaceLocator};

use crate::cli_config::WorkspaceConfig;

/// Re-install an environment, both updating the lockfile and re-installing the environment.
///
/// This command reinstalls an environment, if the lockfile is not up-to-date it will be updated.
/// If packages are specified, only those packages will be reinstalled.
/// Otherwise the whole environment will be reinstalled.
///
/// `pixi reinstall` only re-installs one environment at a time,
/// if you have multiple environments you can select the right one with the `--environment` flag.
/// If you don't provide an environment, the `default` environment will be re-installed.
///
/// If you want to re-install all environments, you can use the `--all` flag.
#[derive(Parser, Debug)]
pub struct Args {
    /// Specifies the package that should be reinstalled.
    /// If no package is given, the whole environment will be reinstalled.
    #[arg(value_name = "PACKAGE")]
    packages: Option<Vec<String>>,

    #[clap(flatten)]
    pub project_config: WorkspaceConfig,

    #[clap(flatten)]
    pub lock_file_usage: crate::LockFileUsageConfig,

    /// The environment to install.
    #[arg(long, short)]
    pub environment: Option<Vec<String>>,

    #[clap(flatten)]
    pub config: ConfigCli,

    /// Install all environments.
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

    let reinstall_packages = args
        .packages
        .map(|p| p.into_iter().collect())
        .map(ReinstallPackages::Some)
        .unwrap_or(ReinstallPackages::All);

    let mut installed_envs = Vec::with_capacity(envs.len());
    for env in envs {
        let environment = workspace.environment_from_name_or_env_var(Some(env))?;

        // Update the prefix by installing all packages
        get_update_lock_file_and_prefix(
            &environment,
            UpdateMode::Revalidate,
            UpdateLockFileOptions {
                lock_file_usage: args.lock_file_usage.to_usage(),
                no_install: false,
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
            },
            reinstall_packages.clone(),
            &InstallFilter::default(),
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
            "{}The {} environment has been re-installed{}.",
            console::style(console::Emoji("✔ ", "")).green(),
            installed_envs[0].fancy_display(),
            detached_envs_message
        );
    } else {
        eprintln!(
            "{}The following environments have been re-installed: {}\t{}",
            console::style(console::Emoji("✔ ", "")).green(),
            installed_envs.iter().map(|n| n.fancy_display()).join(", "),
            detached_envs_message
        );
    }

    Ok(())
}
