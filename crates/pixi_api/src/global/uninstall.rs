use pixi_global::{EnvironmentName, Project, StateChanges};

use crate::global::utils::revert_environment_after_error;

/// Uninstall the environment `env_name`. On failure the environment is
/// reverted and `project` is left untouched.
pub async fn uninstall_environment(
    project: &mut Project,
    env_name: &EnvironmentName,
) -> miette::Result<StateChanges> {
    let mut modified_project = project.clone();
    match apply_changes(env_name, &mut modified_project).await {
        Ok(state_changes) => {
            // Only advance the project when successful
            *project = modified_project;
            Ok(state_changes)
        }
        Err(err) => {
            // Revert any partial change for this environment
            if let Err(revert_err) = revert_environment_after_error(env_name, project).await {
                tracing::warn!("Reverting of the operation failed");
                tracing::info!("Reversion error: {:?}", revert_err);
            }
            Err(err)
        }
    }
}

async fn apply_changes(
    env_name: &EnvironmentName,
    project_modified: &mut Project,
) -> miette::Result<StateChanges> {
    let mut state_changes = StateChanges::new_with_env(env_name.clone());
    state_changes |= project_modified.remove_environment(env_name).await?;

    project_modified.manifest.save().await?;
    Ok(state_changes)
}
