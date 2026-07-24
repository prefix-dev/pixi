use std::path::PathBuf;

use clap::Parser;
use rattler_conda_types::Platform;

use crate::cli_config::NoInstallConfig;

/// Resolve a script environment and write its adjacent Pixi lock file.
#[derive(Debug, Parser)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub config: pixi_config::ConfigCli,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,

    /// Script whose environment should be locked.
    pub path: PathBuf,

    /// Platform to include in the lock file. May be specified more than once.
    #[arg(long, value_name = "PLATFORM")]
    pub platform: Vec<Platform>,

    /// Output the changes in JSON format.
    #[arg(long)]
    pub json: bool,

    /// Exit unsuccessfully if the lock file would change.
    #[arg(long)]
    pub check: bool,

    /// Compute the lock file without writing it.
    #[arg(long)]
    pub dry_run: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    crate::lock::execute(crate::lock::Args {
        config_source: args.config_source,
        script: Some(args.path),
        script_platforms: (!args.platform.is_empty()).then_some(args.platform),
        config: args.config,
        no_install_config: args.no_install_config,
        json: args.json,
        check: args.check,
        dry_run: args.dry_run,
        ..Default::default()
    })
    .await
}
