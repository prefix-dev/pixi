use crate::global::{self};
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

    let updated_env = global::sync(&config, args.assume_yes).await?;

    if !updated_env {
        eprintln!(
            "{} Nothing to do. The pixi global installation is already up-to-date",
            console::style(console::Emoji("✔ ", "")).green()
        );
    }

    Ok(())
}
