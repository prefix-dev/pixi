use crate::global::{EnvironmentAction, report_failed_environment, report_failed_environments};
use clap::Parser;
use fancy_display::FancyDisplay;
use pixi_config::{Config, ConfigCli};
use pixi_global::report::EnvStatus;

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

    // Prune environments that are not listed
    let state_change = project.prune_old_environments().await?;

    #[cfg(unix)]
    {
        // Prune broken completions
        let completions_dir = pixi_global::completions::CompletionsDir::from_env().await?;
        completions_dir.prune_old_completions()?;
    }

    if state_change.has_changed() {
        state_change.report(&project).await;
    }

    // Remove broken files
    if let Err(err) = project.remove_broken_files().await {
        tracing::warn!("Couldn't remove broken files\n{err:?}")
    }

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
    // they write into directories shared across all environments. Drive a spinner
    // over the loop so a sync that installs nothing still shows progress instead of
    // appearing frozen while executables are (re)exposed and trampolines rebuilt
    // (#6658). Reports use `pixi_progress::println!`, which suspends the spinner, so
    // change output stays readable above it.
    let total = env_names.len();
    let (unchanged, errors) =
        pixi_progress::await_in_progress("Syncing global environments", |pb| async move {
            // `sync` sweeps every environment rather than the ones the user
            // named, so the unchanged ones are counted instead of getting a
            // block of their own.
            let mut unchanged = 0;
            let mut errors = Vec::new();
            for (index, (env_name, install_result)) in
                env_names.iter().zip(install_results).enumerate()
            {
                pb.set_message(format!(
                    "syncing {} ({}/{total})",
                    env_name.fancy_display(),
                    index + 1
                ));
                let result = match install_result {
                    Ok(mut state_changes) => {
                        match project.sync_environment_expose(env_name).await {
                            Ok(expose_changes) => {
                                state_changes |= expose_changes;
                                Ok(state_changes)
                            }
                            Err(err) => Err(err),
                        }
                    }
                    Err(err) => Err(err),
                };
                match result {
                    Ok(state_change) => {
                        for env_report in state_change.reports_or_warn(&project).await {
                            if env_report.status == Some(EnvStatus::Unchanged) {
                                unchanged += 1;
                            } else {
                                pixi_global::report::print(&env_report);
                            }
                        }
                    }
                    Err(err) => {
                        report_failed_environment(env_name);
                        errors.push((env_name.clone(), err));
                    }
                }
            }
            (unchanged, errors)
        })
        .await;

    pixi_global::report::print_unchanged_summary(unchanged);
    pixi_global::report::print_nothing_to_do();

    report_failed_environments(EnvironmentAction::Sync, errors)
}
