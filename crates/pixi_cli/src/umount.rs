use std::path::PathBuf;

use clap::Parser;
use miette::miette;
use pixi_config::{Config, ConfigCli};
use pixi_core::WorkspaceLocator;
use pixi_core::environment::mount_sidecar;

use crate::cli_config::WorkspaceConfig;

/// Unmount a pixi environment virtual filesystem.
///
/// Terminates the mount sidecar process and unmounts the environment directory.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    config: ConfigCli,

    /// The environment to unmount.
    #[arg(long, short)]
    environment: Option<String>,

    /// Mount point override. Defaults to the environment directory.
    #[arg(long)]
    mount_point: Option<PathBuf>,

    /// Unmount even if other pixi processes are currently using the mount.
    #[arg(long)]
    force: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::from(args.config);
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(config);

    let environment = workspace.environment_from_name_or_env_var(args.environment)?;
    let env_dir = environment.dir();
    let mount_point = args.mount_point.unwrap_or_else(|| env_dir.clone());

    // Refuse to unmount out from under active clients unless forced. This is
    // advisory (see `clients_attached`); a rare false positive is resolved by
    // passing `--force`.
    if !args.force && mount_sidecar::clients_attached(&mount_point) {
        return Err(miette!(
            "environment mount at {} is in use by another pixi process; \
             pass --force to unmount anyway",
            mount_point.display()
        ));
    }

    // Terminate the sidecar (SIGTERM → SIGKILL), remove the coordination record,
    // and force-unmount any residue — unmount happens before the record is
    // removed, and the transport is taken from the record. Runs on a blocking
    // thread because it may sleep while waiting for the sidecar to exit.
    let mp = mount_point.clone();
    let torn_down = tokio::task::spawn_blocking(move || mount_sidecar::teardown_mount(&mp))
        .await
        .map_err(|e| miette!("umount task failed: {e}"))??;

    if torn_down {
        eprintln!("Unmounted {}", mount_point.display());
    } else {
        eprintln!("Not mounted: {}", mount_point.display());
    }
    Ok(())
}
