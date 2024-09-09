use crate::global::{self};
use clap::Parser;
use pixi_config::{Config, ConfigCli};

/// Sync global manifest with installed environments
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,
    #[clap(flatten)]
    config: ConfigCli,
}

/// Sync global manifest with installed environments
pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);

    global::sync(&config, args.assume_yes).await
}
