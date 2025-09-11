use crate::global::revert_environment_after_error;
use clap::Parser;
use fancy_display::FancyDisplay;
use miette::Report;
use pixi_config::{Config, ConfigCli};
use pixi_global::StateChanges;
use pixi_global::{EnvironmentName, Project};

/// Uninstalls environments from the global environment.
///
/// Example: `pixi global uninstall pixi-pack rattler-build`
#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    /// Specifies the environments that are to be removed.
    #[arg(num_args = 1.., required = true)]
    environment: Vec<EnvironmentName>,

    #[clap(flatten)]
    config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = pixi_global::Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(
        env_name: &EnvironmentName,
        project_modified: &mut Project,
    ) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::new_with_env(env_name.clone());
        state_changes |= project_modified.remove_environment(env_name).await?;

        project_modified.manifest.save().await?;
        Ok(state_changes)
    }

    let mut errors: Vec<(EnvironmentName, Report)> = Vec::new();
    let mut last_updated_project = project_original;
    for env_name in &args.environment {
        let mut project = last_updated_project.clone();
        match apply_changes(env_name, &mut project).await {
            Ok(state_changes) => {
                state_changes.report();
                // Only advance the project when successful
                last_updated_project = project;
            }
            Err(err) => {
                // Revert any partial change for this environment, then continue
                if let Err(revert_err) =
                    revert_environment_after_error(env_name, &last_updated_project).await
                {
                    tracing::warn!("Reverting of the operation failed");
                    tracing::info!("Reversion error: {:?}", revert_err);
                }
                errors.push((env_name.clone(), err));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        for (env_name, err) in errors {
            tracing::warn!(
                "Couldn't remove environment {}\n{err:?}",
                env_name.fancy_display()
            );
        }
        Err(miette::miette!("Some environments couldn't be removed."))
    }
}
