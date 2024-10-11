use crate::global;
use clap::Parser;
use pixi_config::{Config, ConfigCli};

/// Sync global manifest with installed environments
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    config: ConfigCli,
}

/// Sync global manifest with installed environments
pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let mut project = global::Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    let state_changes = project.sync().await?;

    if state_changes.has_changed() {
        state_changes.report();
    } else {
        eprintln!(
            "{}Nothing to do. The pixi global installation is already up-to-date.",
            console::style(console::Emoji("✔ ", "")).green()
        );
    }

    Ok(())
}
