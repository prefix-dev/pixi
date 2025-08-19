use crate::global::global_specs::GlobalSpecs;
use crate::global::revert_environment_after_error;

use clap::Parser;
use pixi_config::{Config, ConfigCli};
use pixi_global::project::GlobalSpec;
use pixi_global::{EnvironmentName, Mapping, Project, StateChange, StateChanges};

/// Adds dependencies to an environment
///
/// Example:
///
/// - `pixi global add --environment python numpy`
/// - `pixi global add --environment my_env pytest pytest-cov --expose pytest=pytest`
#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    /// Specifies the package that should be added to the environment.
    #[clap(flatten)]
    packages: GlobalSpecs,

    /// Specifies the environment that the dependencies need to be added to.
    #[clap(short, long, required = true)]
    environment: EnvironmentName,

    /// Add one or more mapping which describe which executables are exposed.
    /// The syntax is `exposed_name=executable_name`, so for example `python3.10=python`.
    /// Alternatively, you can input only an executable_name and `executable_name=executable_name` is assumed.
    #[arg(long)]
    expose: Vec<Mapping>,

    #[clap(flatten)]
    config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    if project_original.environment(&args.environment).is_none() {
        miette::bail!(
            "Environment {} doesn't exist. You can create a new environment with `pixi global install`.",
            &args.environment
        );
    }

    async fn apply_changes(
        env_name: &EnvironmentName,
        specs: &[GlobalSpec],
        expose: &[Mapping],
        project: &mut Project,
    ) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::new_with_env(env_name.clone());

        // Add specs to the manifest
        for spec in specs {
            project.manifest.add_dependency(env_name, spec)?;
        }

        // Add expose mappings to the manifest
        for mapping in expose {
            project.manifest.add_exposed_mapping(env_name, mapping)?;
        }

        // Sync environment
        let sync_changes = project.sync_environment(env_name, None).await?;

        // Figure out added packages and their corresponding versions from EnvironmentUpdate
        let requested_package_names: Vec<_> =
            specs.iter().map(|spec| spec.name().clone()).collect();

        // Extract EnvironmentUpdate from sync changes if present
        if let Some(changes_for_env) = sync_changes.changes_for_env(env_name) {
            for change in changes_for_env {
                if let StateChange::UpdatedEnvironment(environment_update) = change {
                    let user_requested_changes =
                        environment_update.user_requested_changes(&requested_package_names);

                    // Convert to StateChange::AddedPackage for packages that were installed or upgraded
                    state_changes
                        .add_packages_from_install_changes(
                            env_name,
                            user_requested_changes,
                            project,
                        )
                        .await?;
                    break;
                }
            }
        }

        // Add the sync changes
        state_changes |= sync_changes;

        state_changes |= project.sync_completions(env_name).await?;

        project.manifest.save().await?;

        Ok(state_changes)
    }

    let specs = args
        .packages
        .to_global_specs(
            project_original.global_channel_config(),
            &project_original.root,
            &project_original,
        )
        .await?;

    let mut project_modified = project_original.clone();

    match apply_changes(
        &args.environment,
        &specs,
        args.expose.as_slice(),
        &mut project_modified,
    )
    .await
    {
        Ok(state_changes) => {
            state_changes.report();
            Ok(())
        }
        Err(err) => {
            if let Err(revert_err) =
                revert_environment_after_error(&args.environment, &project_original).await
            {
                tracing::warn!("Reverting of the operation failed");
                tracing::info!("Reversion error: {:?}", revert_err);
            }
            Err(err)
        }
    }
}
