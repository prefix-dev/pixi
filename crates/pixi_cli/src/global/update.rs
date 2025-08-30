use crate::global::revert_environment_after_error;
use clap::Parser;
use fancy_display::FancyDisplay;
use pixi_config::{Config, ConfigCli};
use pixi_global::StateChanges;
use pixi_global::common::{EnvironmentUpdate, InstallChange, check_all_exposed};
use pixi_global::project::ExposedType;
use pixi_global::{EnvironmentName, Project, StateChange};
use serde::Serialize;

/// Updates environments in the global environment.
#[derive(Parser, Debug, Clone)]
pub struct Args {
    /// Specifies the environments that are to be updated.
    environments: Option<Vec<EnvironmentName>>,

    /// Don't actually update any environment.
    #[clap(short = 'n', long)]
    pub dry_run: bool,

    /// Output the changes in JSON format.
    #[clap(long)]
    pub json: bool,

    #[clap(flatten)]
    config: ConfigCli,
}

/// JSON representation of a package change in a global environment update
#[derive(Serialize, Clone, Debug)]
pub struct JsonPackageChange {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<String>,
    change_type: String,
}

/// JSON representation of environment changes during global update
#[derive(Serialize, Clone, Debug)]
pub struct JsonEnvironmentUpdate {
    environment: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    package_changes: Vec<JsonPackageChange>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    exposed_changes: Vec<String>,
    status: String,
}

/// JSON output for global update command
#[derive(Serialize, Clone, Debug)]
pub struct GlobalUpdateJsonOutput {
    version: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    environment_updates: Vec<JsonEnvironmentUpdate>,
}

/// Custom reporting for dry-run mode
fn report_dry_run_environment_update(
    env_name: &EnvironmentName,
    environment_update: &EnvironmentUpdate,
) {
    if environment_update.is_empty() {
        return;
    }

    // Get the package changes
    let changes = environment_update.changes();
    let env_dependencies = environment_update.current_packages();

    // Separate top-level changes (similar to StateChanges::report_update_changes)
    let mut top_level_changes: Vec<_> = changes
        .iter()
        .filter(|(package_name, change)| {
            env_dependencies.contains(package_name) && !change.is_transitive()
        })
        .collect();

    top_level_changes.sort_by(|(name1, _), (name2, _)| name1.cmp(name2));

    match top_level_changes.len().cmp(&1) {
        std::cmp::Ordering::Equal => {
            let (package, install_change) = top_level_changes[0];
            let changes = console::style(package.as_normalized()).green();
            let version_string = install_change
                .version_fancy_display()
                .map(|version| format!("={version}"))
                .unwrap_or_default();

            eprintln!(
                "{}Would update package {}{} in environment {}.",
                console::style(console::Emoji("✔ ", "")).green(),
                changes,
                version_string,
                env_name.fancy_display()
            );
        }
        std::cmp::Ordering::Greater => {
            eprintln!(
                "{}Would update packages in environment {}:",
                console::style(console::Emoji("✔ ", "")).green(),
                env_name.fancy_display()
            );
            for (package, install_change) in top_level_changes {
                let package_fancy = console::style(package.as_normalized()).green();
                let change_fancy = install_change
                    .version_fancy_display()
                    .map(|version| format!(" {version}"))
                    .unwrap_or_default();
                eprintln!("    - {package_fancy}{change_fancy}");
            }
        }
        std::cmp::Ordering::Less => {
            // No packages to update (len == 0)
        }
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = pixi_global::Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(
        env_name: &EnvironmentName,
        project: &mut Project,
        dry_run: bool,
        json_output: bool,
    ) -> miette::Result<(StateChanges, Option<EnvironmentUpdate>)> {
        let mut state_changes = StateChanges::default();

        let should_check_for_updates = true;

        let mut dry_run_environment_update = None;

        // Determine the expose type BEFORE any updates
        let expose_type = if !dry_run && !json_output {
            let environment = project.environment(env_name).ok_or_else(|| {
                miette::miette!("Environment {} not found", env_name.fancy_display())
            })?;

            if let Ok(env_binaries) = project.executables_of_direct_dependencies(env_name).await {
                if check_all_exposed(&env_binaries, &environment.exposed) {
                    Some(ExposedType::All)
                } else {
                    // user manually configured, don't modify
                    None
                }
            } else if environment.exposed.is_empty() {
                Some(ExposedType::All)
            } else {
                // has existing exposure config, don't modify
                None
            }
        } else {
            None
        };

        if should_check_for_updates {
            if dry_run || json_output {
                // dry-run mode: performs solving only
                let environment_update = project.solve_for_dry_run(env_name).await?;

                // Only add to state changes if there are actual changes
                if !environment_update.is_empty() {
                    dry_run_environment_update = Some(environment_update.clone());
                    state_changes.insert_change(
                        env_name,
                        StateChange::UpdatedEnvironment(environment_update),
                    );
                }
            } else {
                // Normal mode: actually install
                let environment_update = project.install_environment(env_name).await?;
                state_changes.insert_change(
                    env_name,
                    StateChange::UpdatedEnvironment(environment_update),
                );
            }
        }

        if !dry_run && !json_output {
            // Always prune invalid/outdated mappings
            project
                .sync_exposed_names(env_name, ExposedType::Nothing)
                .await?;

            if let Some(expose_type) = expose_type {
                // When auto-exposing, add new binaries to the manifest
                project.sync_exposed_names(env_name, expose_type).await?;
            }

            // Expose or prune executables of the new environment (always)
            state_changes |= project
                .expose_executables_from_environment(env_name)
                .await?;

            // Sync completions (always)
            state_changes |= project.sync_completions(env_name).await?;
        }

        Ok((state_changes, dry_run_environment_update))
    }

    // Update all environments if the user did not specify any
    let env_names = match args.environments {
        Some(env_names) => env_names,
        None => {
            if !args.dry_run {
                // prune old environments and completions in non-dry-run mode
                let state_changes = project_original.prune_old_environments().await?;
                state_changes.report();
                #[cfg(unix)]
                {
                    let completions_dir =
                        pixi_global::completions::CompletionsDir::from_env().await?;
                    completions_dir.prune_old_completions()?;
                }
            }
            project_original.environments().keys().cloned().collect()
        }
    };

    // Apply changes to each environment
    let mut last_updated_project = project_original;
    let mut all_state_changes = Vec::new();
    let mut all_environment_updates = Vec::new();

    for env_name in env_names {
        let mut project = last_updated_project.clone();

        match apply_changes(&env_name, &mut project, args.dry_run, args.json).await {
            Ok((state_changes, dry_run_env_update)) => {
                // Collect changes for final summary or JSON output
                all_state_changes.push((env_name.clone(), state_changes.clone()));
                all_environment_updates.push((env_name.clone(), dry_run_env_update.clone()));

                // Report immediately if not in JSON mode
                if !args.json {
                    if args.dry_run {
                        // custom messaging for dry-run mode
                        if state_changes.has_changed() {
                            eprintln!(
                                "{}Would update environment {}:",
                                console::style(console::Emoji("🔍 ", "")).yellow(),
                                env_name.fancy_display()
                            );
                            if let Some(env_update) = dry_run_env_update {
                                report_dry_run_environment_update(&env_name, &env_update);
                            }
                        }
                    } else {
                        // Normal mode: use standard reporting
                        state_changes.report();
                    }
                }
            }
            Err(err) => {
                if !args.dry_run && !args.json {
                    revert_environment_after_error(&env_name, &last_updated_project).await?;
                }
                return Err(err);
            }
        }

        // update project state if not in dry-run mode and not in JSON mode
        if !args.dry_run && !args.json {
            last_updated_project = project;
        }
    }

    // Output final results
    if args.json {
        output_json_results(all_environment_updates)?;
    } else if args.dry_run {
        let total_changed = all_state_changes
            .iter()
            .filter(|(_, changes)| changes.has_changed())
            .count();

        if total_changed == 0 {
            eprintln!(
                "{}No environments need updating.",
                console::style(console::Emoji("✔ ", "")).green()
            );
        } else {
            eprintln!(
                "{}Dry-run complete. {} environment(s) would be updated. No changes were made.",
                console::style(console::Emoji("✔ ", "")).green(),
                total_changed
            );
        }
    }

    if !args.dry_run && !args.json {
        last_updated_project.manifest.save().await?;
    }

    Ok(())
}

/// Convert environment updates to JSON output format
fn output_json_results(
    all_environment_updates: Vec<(EnvironmentName, Option<EnvironmentUpdate>)>,
) -> miette::Result<()> {
    let mut environment_updates = Vec::new();

    for (env_name, env_update) in all_environment_updates {
        let mut package_changes = Vec::new();
        let exposed_changes = Vec::new();
        let mut status = "unchanged".to_string();

        if let Some(env_update) = env_update {
            if !env_update.is_empty() {
                status = "updated".to_string();

                // Extract real package changes from EnvironmentUpdate
                for (package_name, install_change) in env_update.changes() {
                    let (before, after, change_type) = match install_change {
                        InstallChange::Installed(version) => {
                            (None, Some(version.to_string()), "installed".to_string())
                        }
                        InstallChange::Upgraded(old_version, new_version) => (
                            Some(old_version.to_string()),
                            Some(new_version.to_string()),
                            "upgraded".to_string(),
                        ),
                        InstallChange::TransitiveUpgraded(old_version, new_version) => (
                            Some(old_version.to_string()),
                            Some(new_version.to_string()),
                            "transitive_upgraded".to_string(),
                        ),
                        InstallChange::Reinstalled(old_version, new_version) => (
                            Some(old_version.to_string()),
                            Some(new_version.to_string()),
                            "reinstalled".to_string(),
                        ),
                        InstallChange::Removed => (None, None, "removed".to_string()),
                    };

                    package_changes.push(JsonPackageChange {
                        name: package_name.as_normalized().to_string(),
                        before,
                        after,
                        change_type,
                    });
                }
            }
        }

        environment_updates.push(JsonEnvironmentUpdate {
            environment: env_name.to_string(),
            package_changes,
            exposed_changes,
            status,
        });
    }

    let json_output = GlobalUpdateJsonOutput {
        version: 1,
        environment_updates,
    };

    let json_string = serde_json::to_string_pretty(&json_output)
        .map_err(|e| miette::miette!("Failed to serialize JSON output: {}", e))?;

    println!("{}", json_string);
    Ok(())
}
