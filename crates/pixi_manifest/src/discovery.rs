use std::{path::PathBuf, sync::Arc};

use miette::{Diagnostic, NamedSource};
use pixi_consts::consts;
use thiserror::Error;
use toml_span::Deserialize;

use crate::{
    pyproject::PyProjectManifest,
    toml::{ExternalWorkspaceProperties, TomlManifest, Warning},
    utils::WithSourceCode,
    ManifestKind, ManifestProvenance, ManifestSource, PackageManifest, TomlError, WithProvenance,
    WorkspaceManifest,
};

/// A helper struct to discover the workspace manifest in a directory tree from
/// a given path.
pub struct WorkspaceDiscoverer {
    /// The current path
    current_path: PathBuf,
}

/// A workspace discovered by calling [`WorkspaceDiscoverer::discover`].
#[derive(Debug)]
pub struct DiscoveredWorkspace {
    /// The discovered workspace manifest
    pub workspace_manifest: WithProvenance<WorkspaceManifest>,

    /// Optionally, if the workspace manifest itself contains a package
    /// manifest, it is included here.
    pub workspace_package_manifest: Option<WithProvenance<PackageManifest>>,

    /// Any warnings that were encountered during the discovery process.
    pub warnings: Vec<WithSourceCode<Warning, NamedSource<Arc<str>>>>,
}

#[derive(Debug, Error, Diagnostic)]
pub enum WorkspaceDiscoveryError {
    #[error(transparent)]
    IO(#[from] std::io::Error),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(#[from] WithSourceCode<TomlError, NamedSource<Arc<str>>>),
}

impl WorkspaceDiscoverer {
    /// Constructs a new instance from the current path.
    pub fn new(current_path: PathBuf) -> Self {
        Self { current_path }
    }

    pub fn discover(self) -> Result<Option<DiscoveredWorkspace>, WorkspaceDiscoveryError> {
        // Ensure the initial search directory exists.
        if !self.current_path.is_dir() {
            return Ok(None);
        }

        // Walk up the directory tree until we find a workspace manifest.
        let mut current_path = Some(self.current_path.as_path());
        while let Some(manifest_dir_path) = current_path {
            let path_diff = pathdiff::diff_paths(manifest_dir_path, self.current_path.as_path())
                .unwrap_or_else(|| manifest_dir_path.to_path_buf());
            let file_name = path_diff.to_string_lossy();

            // Prepare the next directory to search.
            current_path = manifest_dir_path.parent();

            // Check if a pixi.toml file exists in the current directory.
            let pixi_toml_path = manifest_dir_path.join(consts::PROJECT_MANIFEST);
            let pyproject_toml_path = manifest_dir_path.join(consts::PYPROJECT_MANIFEST);
            let provenance = if pixi_toml_path.is_file() {
                ManifestProvenance::new(pixi_toml_path, ManifestKind::Pixi)
            } else if pyproject_toml_path.is_file() {
                ManifestProvenance::new(pixi_toml_path, ManifestKind::Pyproject)
            } else {
                // Continue the search
                continue;
            };

            // Read the contents of the manifest file.
            let contents = provenance.read()?.map(Arc::<str>::from);

            // Cheap check to see if the manifest contains a pixi section.
            if let ManifestSource::PyProjectToml(source) = &contents {
                if !source.contains("[tool.pixi") {
                    continue;
                }
            }

            let source = contents.into_named(file_name);

            // Parse the TOML from the manifest
            let mut toml = match toml_span::parse(source.inner()) {
                Ok(toml) => toml,
                Err(e) => {
                    return Err(WithSourceCode {
                        error: TomlError::from(e),
                        source,
                    }
                    .into())
                }
            };

            // Parse the workspace manifest.
            let parsed_manifest = match provenance.kind {
                ManifestKind::Pixi => TomlManifest::deserialize(&mut toml)
                    .map_err(TomlError::from)
                    .and_then(|manifest| {
                        manifest.into_workspace_manifest(ExternalWorkspaceProperties::default())
                    }),
                ManifestKind::Pyproject => PyProjectManifest::deserialize(&mut toml)
                    .map_err(TomlError::from)
                    .and_then(|manifest| manifest.into_manifests()),
            };

            let (workspace_manifest, package_manifest, warnings) = match parsed_manifest {
                Ok(parsed_manifest) => parsed_manifest,
                Err(error) => return Err(WithSourceCode { error, source }.into()),
            };

            return Ok(Some(DiscoveredWorkspace {
                workspace_package_manifest: package_manifest.map(|package_manifest| {
                    WithProvenance::new(package_manifest, provenance.clone())
                }),
                workspace_manifest: WithProvenance::new(workspace_manifest, provenance),
                warnings: warnings
                    .into_iter()
                    .map(|warning| WithSourceCode {
                        error: warning,
                        source: source.clone(),
                    })
                    .collect(),
            }));
        }

        Ok(None)
    }
}
