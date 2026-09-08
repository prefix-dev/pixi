use crate::global::{
    EnvironmentAction, report_failed_environment, report_failed_environments,
    revert_environment_after_error,
};
use clap::Parser;
use fancy_display::FancyDisplay;
use indexmap::IndexMap;
use miette::{Report, WrapErr};
use pixi_config::{Config, ConfigCli};
use pixi_global::common::check_all_exposed;
use pixi_global::project::ExposedType;
use pixi_global::report::EnvStatus;
use pixi_global::{EnvironmentName, Project};
use pixi_global::{StateChange, StateChanges};
use pixi_utils::prefix::Executable;
use rattler_conda_types::PackageName;

/// Updates environments in the global environment.
#[derive(Parser, Debug, Clone)]
pub struct Args {
    /// Specifies the environments that are to be updated.
    environments: Option<Vec<EnvironmentName>>,

    #[clap(flatten)]
    config: ConfigCli,
}

/// Result of the parallel install phase for a single environment.
struct EnvInstallResult {
    env_name: EnvironmentName,
    expose_type: ExposedType,
    environment_update: pixi_global::common::EnvironmentUpdate,
}

/// Phase 1: Check sync, determine expose type, and update the environment.
async fn install_and_determine_expose_type(
    project: &Project,
    env_name: &EnvironmentName,
) -> miette::Result<EnvInstallResult> {
    let in_sync = project.environment_in_sync(env_name).await?;

    let pre_update_bins: Option<IndexMap<PackageName, Vec<Executable>>> = if in_sync {
        Some(project.executables_of_direct_dependencies(env_name).await?)
    } else {
        None
    };

    let environment_update = project.install_environment(env_name).await?;

    let env_binaries = match pre_update_bins {
        Some(bins) => bins,
        None => project.executables_of_direct_dependencies(env_name).await?,
    };

    let exposed = &project
        .environment(env_name)
        .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?
        .exposed;

    let expose_type = if check_all_exposed(&env_binaries, exposed) {
        ExposedType::All
    } else {
        ExposedType::Nothing
    };

    Ok(EnvInstallResult {
        env_name: env_name.clone(),
        expose_type,
        environment_update,
    })
}

/// Phase 2 (sequential): Update manifest, sync shortcuts, expose executables and sync completions.
async fn apply_manifest_changes(
    project: &mut Project,
    result: EnvInstallResult,
) -> miette::Result<StateChanges> {
    let mut state_changes = StateChanges::default();

    state_changes.insert_change(
        &result.env_name,
        StateChange::UpdatedEnvironment(result.environment_update),
    );

    project
        .sync_exposed_names(&result.env_name, result.expose_type)
        .await?;

    state_changes |= project.sync_shortcuts(&result.env_name).await?;

    state_changes |= project
        .expose_executables_from_environment(&result.env_name)
        .await?;

    state_changes |= project.sync_completions(&result.env_name).await?;

    Ok(state_changes)
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = pixi_global::Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    // Environments the user named get a block even when nothing changed;
    // a sweep over all of them counts the unchanged ones instead.
    let named_environments = args.environments.is_some();
    let env_names: Vec<EnvironmentName> = match args.environments {
        Some(env_names) => {
            let mut seen = indexmap::IndexSet::new();
            for name in env_names {
                seen.insert(name);
            }
            seen.into_iter().collect()
        }
        None => {
            let state_changes = project_original.prune_old_environments().await?;
            state_changes.report(&project_original).await;
            #[cfg(unix)]
            {
                let completions_dir = pixi_global::completions::CompletionsDir::from_env().await?;
                completions_dir.prune_old_completions()?;
            }
            project_original.environments().keys().cloned().collect()
        }
    };

    // Install all environments in parallel, sharing one dispatcher.
    let project_ref = &project_original;
    let install_results: Vec<miette::Result<EnvInstallResult>> =
        futures::future::join_all(env_names.iter().map(|env_name| async move {
            install_and_determine_expose_type(project_ref, env_name)
                .await
                .wrap_err_with(|| {
                    format!("Couldn't update environment {}", env_name.fancy_display())
                })
        }))
        .await;

    let mut project = project_original;
    project.clear_progress();
    let mut unchanged = 0;
    let mut errors: Vec<(EnvironmentName, Report)> = Vec::new();
    for (env_name, result) in env_names.iter().zip(install_results) {
        let outcome = match result {
            Ok(env_result) => apply_manifest_changes(&mut project, env_result).await,
            Err(err) => Err(err),
        };
        match outcome {
            Ok(state_changes) => {
                for env_report in state_changes.reports_or_warn(&project).await {
                    if !named_environments && env_report.status == Some(EnvStatus::Unchanged) {
                        unchanged += 1;
                    } else {
                        pixi_global::report::print(&env_report);
                    }
                }
            }
            Err(err) => {
                if let Err(revert_err) = revert_environment_after_error(env_name, &project).await {
                    tracing::warn!("Reverting of the operation failed");
                    tracing::info!("Reversion error: {:?}", revert_err);
                }
                report_failed_environment(env_name);
                errors.push((env_name.clone(), err));
            }
        }
    }

    pixi_global::report::print_unchanged_summary(unchanged);
    pixi_global::report::print_nothing_to_do();

    project.manifest.save().await?;
    report_failed_environments(EnvironmentAction::Update, errors)
}
