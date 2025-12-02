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
    let mut project = pixi_global::Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    let mut has_changed = false;

    // Prune environments that are not listed
    let state_change = project.prune_old_environments().await?;

    #[cfg(unix)]
    {
        // Prune broken completions
        let completions_dir = pixi_global::completions::CompletionsDir::from_env().await?;
        completions_dir.prune_old_completions()?;
    }

    if state_change.has_changed() {
        has_changed = true;
        state_change.report();
    }

    // Remove broken files
    if let Err(err) = project.remove_broken_files().await {
        tracing::warn!("Couldn't remove broken files\n{err:?}")
    }

    let mut errors = Vec::new();

    let env_names: Vec<_> = project.environments().keys().cloned().collect();

    for env_name in env_names {
        match project.sync_environment(&env_name, None).await {
            Ok(state_changes) => {
                if state_changes.has_changed() {
                    state_changes.report();
                }
            }
            Err(err) => errors.push((env_name, err)),
        }
    }

    if !has_changed {
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
