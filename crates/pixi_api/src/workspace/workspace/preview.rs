use std::path::PathBuf;

use itertools::Itertools;
use miette::Diagnostic;
use pixi_core::Workspace;
use pixi_manifest::{
    KnownPreviewFlag, LoadManifestsError, ManifestDocument, ManifestDocumentError,
    ManifestProvenance, ManifestRenderError, ProvenanceError, TomlError,
};
use thiserror::Error;

use crate::Interface;
use crate::workspace::workspace::SaveManifestError;

#[derive(Debug, Error, Diagnostic)]
pub enum PreviewFlagsError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Provenance(#[from] ProvenanceError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Document(#[from] ManifestDocumentError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(#[from] TomlError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Render(#[from] ManifestRenderError),

    #[error("the manifest would no longer load without '{flags}'")]
    #[diagnostic(help("pass `--force` to remove anyway"))]
    StillNeeded {
        flags: String,
        #[source]
        #[diagnostic_source]
        source: LoadManifestsError,
    },

    #[error(transparent)]
    #[diagnostic(transparent)]
    Save(#[from] SaveManifestError),
}

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
) -> Result<(), PreviewFlagsError> {
    let provenance = ManifestProvenance::from_path(manifest_path)?;
    let mut document = ManifestDocument::from_provenance(&provenance)?;

    let mut added = Vec::new();
    for flag in flags {
        if document.add_preview_flag(flag.into())? {
            added.push(flag);
        }
    }

    let contents = document.render()?;
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
) -> Result<(), PreviewFlagsError> {
    let provenance = ManifestProvenance::from_path(manifest_path)?;
    let mut document = ManifestDocument::from_provenance(&provenance)?;

    let mut removed = Vec::new();
    for flag in flags {
        if document.remove_preview_flag(flag.into())? {
            removed.push(flag);
        }
    }

    if removed.is_empty() {
        interface
            .success("The preview flags were not enabled.")
            .await;
        return Ok(());
    }

    let contents = document.render()?;
    match Workspace::from_str(&provenance.path, &contents) {
        Err(source) if !force => {
            return Err(PreviewFlagsError::StillNeeded {
                flags: removed.iter().format("', '").to_string(),
                source,
            });
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
async fn save(provenance: &ManifestProvenance, contents: &str) -> Result<(), SaveManifestError> {
    pixi_utils::atomic_write::atomic_write(&provenance.path, contents)
        .await
        .map_err(|source| SaveManifestError {
            path: provenance.path.clone(),
            source,
        })
}
