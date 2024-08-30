use crate::global::{self, BinDir, EnvRoot};
use clap::Parser;
use pixi_config::{Config, ConfigCli};
use pixi_utils::reqwest::build_reqwest_clients;

/// Sync global manifest with installed environments
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    config: ConfigCli,
}

/// Sync global manifest with installed environments
pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project = global::Project::discover()?.with_cli_config(config.clone());

    // Fetch the repodata
    let (_, auth_client) = build_reqwest_clients(Some(&config));

    let gateway = config.gateway(auth_client.clone());

    let env_root = EnvRoot::from_env().await?;
    let bin_dir = BinDir::from_env().await?;

    global::sync(
        &env_root,
        &project,
        &bin_dir,
        &config,
        &gateway,
        &auth_client,
    )
    .await
}
