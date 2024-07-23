use clap::Parser;

use crate::config::{Config, ConfigCli};

/// Syncs the global environments with the manifest
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    config: ConfigCli,
}

/// Install a global command
pub async fn execute(args: Args) -> miette::Result<()> {
    // Figure out what channels we are using
    // let config = Config::with_cli_config(&args.config);
    let manifest = super::manifest::read_global_manifest();
    manifest.setup_envs().await.unwrap();
    Ok(())
}
