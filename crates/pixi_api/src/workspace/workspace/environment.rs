use miette::IntoDiagnostic;
use pixi_core::{
    Workspace,
    workspace::{Environment, WorkspaceMut},
};
use pixi_manifest::EnvironmentName;

use crate::Interface;

pub async fn list(workspace: &Workspace) -> Vec<Environment> {
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
) -> miette::Result<()> {
    let environment_exists = workspace.workspace().environment(&name).is_some();
    if environment_exists && !force {
        if interface.is_cli().await {
            return Err(miette::miette!(
                help = "use --force to overwrite the existing environment",
                "the environment '{}' already exists",
                name
            ));
        } else {
            return Err(miette::miette!("the environment '{}' already exists", name));
        }
    }

    // Add the platforms to the lock-file
    workspace.manifest().add_environment(
        name.as_str().to_string(),
        features,
        solve_group,
        no_default_feature,
    )?;

    // Save the workspace to disk
    let _workspace = workspace.save().await.into_diagnostic()?;

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
) -> miette::Result<()> {
    // Remove the environment
    if !workspace.manifest().remove_environment(name)? {
        // TODO: Add help for names of environments that are close.
        return Err(miette::miette!("Environment {} not found", name));
    }

    workspace.save().await.into_diagnostic()?;

    interface
        .success(&format!("Removed environment {name}"))
        .await;

    Ok(())
}
