use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_core::{Workspace, workspace::WorkspaceMut};
use pixi_manifest::KnownPreviewFeature;

use crate::Interface;

pub async fn list(workspace: &Workspace) -> Vec<KnownPreviewFeature> {
    workspace
        .workspace
        .value
        .workspace
        .preview
        .enabled_features()
}

pub async fn add<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    features: Vec<KnownPreviewFeature>,
) -> miette::Result<()> {
    // Add the features to the manifest
    let added = workspace.manifest().add_preview_features(features)?;

    // Save the manifest on disk
    workspace.save().await.into_diagnostic()?;

    // Report back to the user
    if added.is_empty() {
        interface
            .success("The preview features were already enabled.")
            .await;
    } else {
        interface
            .success(&format!(
                "Added '{}' to the preview features.",
                added.iter().format("', '")
            ))
            .await;
    }

    Ok(())
}

pub async fn remove<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    features: Vec<KnownPreviewFeature>,
) -> miette::Result<()> {
    // Remove the features from the manifest
    let removed = workspace.manifest().remove_preview_features(features)?;

    // Refuse to write a manifest that no longer loads, e.g. removing
    // `pixi-build` while the manifest still contains source dependencies
    let path = workspace.workspace().workspace.provenance.path.clone();
    let contents = workspace.document().render().into_diagnostic()?;
    Workspace::from_str(&path, &contents).map_err(|err| {
        miette::Report::new(err)
            .wrap_err("the workspace still uses the preview feature(s), not removing them")
    })?;

    // Save the manifest on disk
    workspace.save().await.into_diagnostic()?;

    // Report back to the user
    if removed.is_empty() {
        interface
            .success("The preview features were not enabled.")
            .await;
    } else {
        interface
            .success(&format!(
                "Removed '{}' from the preview features.",
                removed.iter().format("', '")
            ))
            .await;
    }

    Ok(())
}
