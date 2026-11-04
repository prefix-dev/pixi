use pixi_global::{
    EnvironmentName, Project, StateChanges,
    project::{ManifestError, RemoveEnvironmentError},
};

use crate::global::utils::revert_environment_after_error;

/// An error from uninstalling a global environment.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum UninstallError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    RemoveEnvironment(#[from] Box<RemoveEnvironmentError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Manifest(#[from] ManifestError),
}

/// Uninstall the environment `env_name`. On failure the environment is
/// reverted and `project` is left untouched.
pub async fn uninstall_environment(
    project: &mut Project,
    env_name: &EnvironmentName,
) -> Result<StateChanges, UninstallError> {
    let mut modified_project = project.clone();
    match apply_changes(env_name, &mut modified_project).await {
        Ok(state_changes) => {
            // Only advance the project when successful
            *project = modified_project;
            Ok(state_changes)
        }
        Err(err) => {
            // Revert any partial change for this environment
            revert_environment_after_error(env_name, project).await;
            Err(err)
        }
    }
}

async fn apply_changes(
    env_name: &EnvironmentName,
    project_modified: &mut Project,
) -> Result<StateChanges, UninstallError> {
    let mut state_changes = StateChanges::new_with_env(env_name.clone());
    state_changes |= project_modified
        .remove_environment(env_name)
        .await
        .map_err(Box::new)?;

    project_modified.manifest.save().await?;
    Ok(state_changes)
}
