use crate::global::{self, StateChanges};
use clap::Parser;
use fancy_display::FancyDisplay;
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
    let project = global::Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    let mut state_changes = StateChanges::default();

    // Prune environments that are not listed
    state_changes |= project.prune_old_environments().await?;

    // Remove broken files
    if let Err(err) = project.remove_broken_files().await {
        tracing::warn!("Couldn't remove broken files\n{err:?}")
    }

    let mut errors = Vec::new();
    for env_name in project.environments().keys() {
        match project.sync_environment(env_name, None).await {
            Ok(state_change) => state_changes |= state_change,
            Err(err) => errors.push((env_name, err)),
        }
    }

    if state_changes.has_changed() {
        state_changes.report();
    } else {
        eprintln!(
            "{}Nothing to do. The pixi global installation is already up-to-date.",
            console::style(console::Emoji("✔ ", "")).green()
        );
    }

    if errors.is_empty() {
        Ok(())
    } else {
        for (env_name, err) in errors {
            tracing::warn!(
                "Couldn't sync environment {}\n{err:?}",
                env_name.fancy_display(),
            );
        }
        Err(miette::miette!("Some environments couldn't be synced."))
    }
}
