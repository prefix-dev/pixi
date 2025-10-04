use clap::Parser;
use pixi_api::WorkspaceContext;
use pixi_api::workspace::ReinstallOptions;
use pixi_config::ConfigCli;
use pixi_core::WorkspaceLocator;
use pixi_core::lock_file::{ReinstallEnvironment, ReinstallPackages};

use crate::cli_config::WorkspaceConfig;
use crate::cli_interface::CliInterface;

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

impl From<Args> for ReinstallOptions {
    fn from(args: Args) -> Self {
        let reinstall_packages = args
            .packages
            .map(|p| p.into_iter().collect())
            .map(ReinstallPackages::Some)
            .unwrap_or(ReinstallPackages::All);

        let mut reinstall_environments = args
            .environment
            .map(|e| e.into_iter().collect())
            .map(ReinstallEnvironment::Some)
            .unwrap_or(ReinstallEnvironment::Default);

        if args.all {
            reinstall_environments = ReinstallEnvironment::All;
        }

        ReinstallOptions {
            reinstall_packages,
            reinstall_environments,
        }
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.project_config.workspace_locator_start())
        .locate()?
        .with_cli_config(args.config.clone());

    let lock_file_usage = args.lock_file_usage.to_usage();
    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace);
    workspace_ctx
        .reinstall(args.into(), lock_file_usage)
        .await?;

    Ok(())
}
