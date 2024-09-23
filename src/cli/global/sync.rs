use crate::global::{self, BinDir, EnvRoot};
use clap::Parser;
use pixi_config::{Config, ConfigCli};

/// Sync global manifest with installed environments
#[derive(Parser, Debug)]
pub struct Args {
    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,
    #[clap(flatten)]
    config: ConfigCli,
}

/// Sync global manifest with installed environments
pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let bin_dir = BinDir::from_env().await?;
    let env_root = EnvRoot::from_env().await?;

    let project = global::Project::discover_or_create(env_root, bin_dir, args.assume_yes)
        .await?
        .with_cli_config(config.clone());

    global::sync(&project, &config).await
}
