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
    let project = pixi_global::Project::discover_or_create()
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

    // Phase 1: install all environments in parallel, sharing one dispatcher.
    let env_names: Vec<_> = project.environments().keys().cloned().collect();
    let install_results = futures::future::join_all(
        env_names
            .iter()
            .map(|env_name| project.sync_environment_install(env_name, None)),
    )
    .await;
    project.clear_progress();

    // Phase 2: expose executables, shortcuts and completions sequentially, since
    // they write into directories shared across all environments.
    for (env_name, install_result) in env_names.iter().zip(install_results) {
        let result = match install_result {
            Ok(mut state_changes) => match project.sync_environment_expose(env_name).await {
                Ok(expose_changes) => {
                    state_changes |= expose_changes;
                    Ok(state_changes)
                }
                Err(err) => Err(err),
            },
            Err(err) => Err(err),
        };
        match result {
            Ok(state_change) => {
                if state_change.has_changed() {
                    has_changed = true;
                    state_change.report();
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
