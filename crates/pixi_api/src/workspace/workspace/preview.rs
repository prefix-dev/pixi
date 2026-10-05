use std::path::PathBuf;

use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_core::Workspace;
use pixi_manifest::{KnownPreviewFlag, ManifestDocument, ManifestProvenance};

use crate::Interface;

pub async fn list(workspace: &Workspace) -> Vec<KnownPreviewFlag> {
    workspace.workspace.value.workspace.preview.enabled_flags()
}

/// Enables the flags by editing the manifest directly: the manifest
/// doesn't have to be valid, only the result is checked, so a manifest
/// that fails to load exactly because a preview flag is missing, e.g.
/// a `[package]` section without `pixi-build`, can still be fixed.
pub async fn add<I: Interface>(
    interface: &I,
    manifest_path: PathBuf,
    flags: Vec<KnownPreviewFlag>,
) -> miette::Result<()> {
    let provenance = ManifestProvenance::from_path(manifest_path).into_diagnostic()?;
    let mut document = ManifestDocument::from_provenance(&provenance)?;

    let mut added = Vec::new();
    for flag in flags {
        if document.add_preview_flag(flag.into()).into_diagnostic()? {
            added.push(flag);
        }
    }

    let contents = document.render().into_diagnostic()?;
    save(&provenance, &contents).await?;

    if added.is_empty() {
        interface
            .success("The preview flags were already enabled.")
            .await;
    } else {
        interface
            .success(&format!(
                "Added '{}' to the preview flags.",
                added.iter().format("', '")
            ))
            .await;
    }

    // The manifest may have other problems the flag doesn't fix
    if let Err(err) = Workspace::from_str(&provenance.path, &contents) {
        interface
            .warning(&format!("The manifest still fails to load: {err}"))
            .await;
    }
    Ok(())
}

/// Disables the flags by editing the manifest directly, like [`add`].
///
/// When the workspace still needs the removed flag(s), e.g. a
/// `[package]` section without `pixi-build`, the manifest is left
/// untouched and an error is returned, unless `force` is set.
pub async fn remove<I: Interface>(
    interface: &I,
    manifest_path: PathBuf,
    flags: Vec<KnownPreviewFlag>,
    force: bool,
) -> miette::Result<()> {
    let provenance = ManifestProvenance::from_path(manifest_path).into_diagnostic()?;
    let mut document = ManifestDocument::from_provenance(&provenance)?;

    let mut removed = Vec::new();
    for flag in flags {
        if document
            .remove_preview_flag(flag.into())
            .into_diagnostic()?
        {
            removed.push(flag);
        }
    }

    if removed.is_empty() {
        interface
            .success("The preview flags were not enabled.")
            .await;
        return Ok(());
    }

    let contents = document.render().into_diagnostic()?;
    match Workspace::from_str(&provenance.path, &contents) {
        Err(err) if !force => {
            return Err(miette::Report::new(err).wrap_err(format!(
                "The manifest would no longer load without '{}'. \
                 Pass `--force` to remove anyway.",
                removed.iter().format("', '")
            )));
        }
        load_result => {
            save(&provenance, &contents).await?;

            interface
                .success(&format!(
                    "Removed '{}' from the preview flags.",
                    removed.iter().format("', '")
                ))
                .await;

            if let Err(err) = load_result {
                interface
                    .warning(&format!("The manifest no longer loads: {err}"))
                    .await;
            }
        }
    }
    Ok(())
}

/// Persists the edited manifest to disk.
async fn save(provenance: &ManifestProvenance, contents: &str) -> miette::Result<()> {
    pixi_utils::atomic_write::atomic_write(&provenance.path, contents)
        .await
        .into_diagnostic()
}
