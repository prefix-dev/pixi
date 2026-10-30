use miette::Diagnostic;
use pixi_core::{
    Workspace,
    workspace::{Environment, WorkspaceMut},
};
use pixi_manifest::{
    EnvironmentEditError, EnvironmentName, LoadManifestsError, NewEnvironment, TomlError,
};
use thiserror::Error;

use crate::Interface;
use crate::workspace::workspace::{SaveManifestError, save};

#[derive(Debug, Error, Diagnostic)]
pub enum EnvironmentError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    LoadWorkspace(#[from] LoadManifestsError),

    #[error("the environment '{name}' already exists")]
    AlreadyExists {
        name: EnvironmentName,
        #[help]
        help: Option<String>,
    },

    // TODO: Add help for names of environments that are close.
    #[error("environment '{0}' not found")]
    NotFound(String),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Edit(#[from] EnvironmentEditError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(#[from] TomlError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Save(#[from] SaveManifestError),
}

pub async fn list(workspace: &Workspace) -> Vec<Environment<'_>> {
    workspace.environments()
}

pub async fn add<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    name: EnvironmentName,
    features: Option<Vec<String>>,
    solve_group: Option<String>,
    no_default_feature: bool,
    force: bool,
) -> Result<(), EnvironmentError> {
    let environment_exists = workspace.workspace().environment(&name).is_some();
    if environment_exists && !force {
        let help = interface
            .is_cli()
            .await
            .then(|| "use --force to overwrite the existing environment".to_string());
        return Err(EnvironmentError::AlreadyExists { name, help });
    }

    // Add the platforms to the lock file
    workspace.manifest().add_environment(
        NewEnvironment::new(name.as_str())
            .with_features(features)
            .with_solve_group(solve_group)
            .with_no_default_feature(no_default_feature),
    )?;

    // Save the workspace to disk
    save(workspace).await?;

    // Report back to the user
    interface
        .success(&format!(
            "{} environment {}",
            if environment_exists {
                "Updated"
            } else {
                "Added"
            },
            name
        ))
        .await;

    Ok(())
}

pub async fn remove<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    name: &str,
) -> Result<(), EnvironmentError> {
    // Remove the environment
    if !workspace.manifest().remove_environment(name)? {
        return Err(EnvironmentError::NotFound(name.to_string()));
    }

    save(workspace).await?;

    interface
        .success(&format!("Removed environment {name}"))
        .await;

    Ok(())
}
