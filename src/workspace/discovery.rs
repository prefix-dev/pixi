use std::{path::PathBuf, sync::Arc};

use itertools::Itertools;
use miette::{Diagnostic, NamedSource, Report};
use pixi_consts::consts;
use pixi_manifest::{
    utils::WithSourceCode, ExplicitManifestError, LoadManifestsError, Manifests, TomlError,
    WarningWithSource, WithWarnings, WorkspaceDiscoveryError,
};
use thiserror::Error;

use crate::workspace::Workspace;

/// Defines where the search for the workspace should start.
#[derive(Debug, Clone, Default)]
pub enum DiscoveryStart {
    /// Start the search from the current directory indicated by
    /// [`std::env::current_dir`].
    #[default]
    CurrentDir,

    /// Start the search from the given directory.
    ///
    /// If a manifest is not found at the specified location the search will
    /// recursively continue to the parent.
    SearchRoot(PathBuf),

    /// Use the manifest file at the given path. Only search for a workspace if
    /// the specified manifest is not a workspace.
    ///
    /// If no manifest is found at the given path the search will abort.
    ExplicitManifest(PathBuf),
}

/// A helper struct that helps discover the workspace root and potentially the
/// "current" package.
#[derive(Default)]
pub struct WorkspaceLocator {
    start: DiscoveryStart,
    with_closest_package: bool,
    emit_warnings: bool,
    consider_environment: bool,
}

#[derive(Debug, Error, Diagnostic)]
pub enum WorkspaceLocatorError {
    /// An IO error occurred while trying to discover the workspace.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Failed to determine the current directory.
    #[error("failed to determine the current directory")]
    CurrentDir(#[source] std::io::Error),

    /// A TOML parsing error occurred while trying to discover the workspace.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(#[from] Box<WithSourceCode<TomlError, NamedSource<Arc<str>>>>),

    /// The workspace could not be located.
    #[error(
        "could not find {project_manifest} or {pyproject_manifest} at directory {0}",
        project_manifest = consts::PROJECT_MANIFEST,
        pyproject_manifest = consts::PYPROJECT_MANIFEST
    )]
    WorkspaceNotFound(PathBuf),

    #[error("unable to canonicalize '{}'", .path.display())]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The manifest file could not be loaded.
    #[error(transparent)]
    #[diagnostic(transparent)]
    ExplicitManifestError(#[from] ExplicitManifestError),
}

impl WorkspaceLocator {
    /// Constructs a new instance tailored for finding the workspace for CLI
    /// commands.
    pub fn for_cli() -> Self {
        Self::default()
            .with_emit_warnings(true)
            .with_consider_environment(true)
    }

    /// Define where the search for the workspace should start.
    pub fn with_search_start(self, start: DiscoveryStart) -> Self {
        Self { start, ..self }
    }

    /// Also search for the closest package in the workspace.
    pub fn with_closest_package(self, with_closest_package: bool) -> Self {
        Self {
            with_closest_package,
            ..self
        }
    }

    /// Set whether to emit warnings that are encountered during the discovery
    /// process.
    pub fn with_emit_warnings(self, emit_warnings: bool) -> Self {
        Self {
            emit_warnings,
            ..self
        }
    }

    /// Whether to consider any environment variables that may be set that could
    /// influence the discovery process.
    pub fn with_consider_environment(self, consider_environment: bool) -> Self {
        Self {
            consider_environment,
            ..self
        }
    }

    /// Called to locate the workspace or error out if none could be located.
    pub fn locate(self) -> Result<Workspace, WorkspaceLocatorError> {
        // Determine the search root
        let discovery_start = match self.start {
            DiscoveryStart::ExplicitManifest(path) => {
                pixi_manifest::DiscoveryStart::ExplicitManifest(path)
            }
            DiscoveryStart::CurrentDir => pixi_manifest::DiscoveryStart::SearchRoot(
                std::env::current_dir().map_err(WorkspaceLocatorError::CurrentDir)?,
            ),
            DiscoveryStart::SearchRoot(path) => pixi_manifest::DiscoveryStart::SearchRoot(path),
        };

        let discovery_source = discovery_start.root().to_path_buf();

        // Discover the workspace manifest for the current path.
        let workspace_manifests = match pixi_manifest::WorkspaceDiscoverer::new(discovery_start)
            .with_closest_package(self.with_closest_package)
            .discover()
        {
            Ok(manifests) => manifests,
            Err(WorkspaceDiscoveryError::Toml(err)) => {
                return Err(WorkspaceLocatorError::Toml(err))
            }
            Err(WorkspaceDiscoveryError::Io(err)) => return Err(WorkspaceLocatorError::Io(err)),
            Err(WorkspaceDiscoveryError::ExplicitManifestError(err)) => {
                return Err(WorkspaceLocatorError::ExplicitManifestError(err))
            }
            Err(WorkspaceDiscoveryError::Canonicalize(source, path)) => {
                return Err(WorkspaceLocatorError::Canonicalize { path, source })
            }
        };

        // Extract the warnings from the discovered workspace.
        let (mut workspace_manifests, mut warnings) = match workspace_manifests {
            Some(WithWarnings {
                value: manifests,
                warnings,
            }) => (Some(manifests), warnings),
            None => (None, Vec::new()),
        };

        // Take into consideration any environment variables that may be set.
        if self.consider_environment {
            if let Some(WithWarnings {
                value: manifests,
                warnings: mut env_warnings,
            }) =
                Self::apply_environment_overrides(workspace_manifests.take(), self.emit_warnings)?
            {
                warnings.append(&mut env_warnings);
                workspace_manifests = Some(manifests);
            }
        }

        // Early out if discovery failed.
        let Some(discovered_manifests) = workspace_manifests else {
            return Err(WorkspaceLocatorError::WorkspaceNotFound(discovery_source));
        };

        // Emit any warnings that were encountered during the discovery process.
        if self.emit_warnings && !warnings.is_empty() {
            tracing::warn!(
                "Encountered {} warning{} while parsing the manifest:\n{}",
                warnings.len(),
                if warnings.len() == 1 { "" } else { "s" },
                warnings
                    .into_iter()
                    .map(Report::from)
                    .format_with("\n", |w, f| f(&format_args!("{:?}", w)))
            );
        }

        Ok(Workspace::from_manifests(discovered_manifests))
    }

    /// Apply any environment overrides to a potentially discovered workspace.
    fn apply_environment_overrides(
        discovered_workspace: Option<Manifests>,
        emit_warnings: bool,
    ) -> Result<Option<WithWarnings<Manifests, WarningWithSource>>, WorkspaceLocatorError> {
        let env_manifest_path = std::env::var("PIXI_PROJECT_MANIFEST")
            .map(PathBuf::from)
            .ok();

        // Warn the user if they are currently in a shell of another workspace.
        if let Some(workspace_manifests) = &discovered_workspace {
            let discovered_manifest_path = &workspace_manifests.workspace.provenance.path;
            let in_shell = std::env::var("PIXI_IN_SHELL").is_ok();
            if let Some(env_manifest_path) = env_manifest_path {
                if &env_manifest_path != discovered_manifest_path && in_shell && emit_warnings {
                    tracing::warn!(
                            "Using local manifest {} rather than {} from environment variable `PIXI_PROJECT_MANIFEST`",
                            discovered_manifest_path.display(),
                            env_manifest_path.display(),
                        );
                }
            }
        // Else, if we didn't find a workspace manifest, but we there is an
        // active one set in the environment, we try to use that instead.
        } else if let Some(env_manifest_path) = env_manifest_path {
            match Manifests::from_workspace_manifest_path(env_manifest_path.clone()) {
                Ok(workspace) => return Ok(Some(workspace)),
                Err(LoadManifestsError::Io(err)) => return Err(WorkspaceLocatorError::Io(err)),
                Err(LoadManifestsError::Toml(err)) => return Err(WorkspaceLocatorError::Toml(err)),
                Err(LoadManifestsError::ProvenanceError(err)) => {
                    return Err(WorkspaceLocatorError::ExplicitManifestError(
                        ExplicitManifestError::InvalidManifest(err),
                    ))
                }
            }
        }

        Ok(discovered_workspace.map(WithWarnings::from))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_workspace_locator() {
        let workspace_locator = WorkspaceLocator::default();
        let workspace = workspace_locator.locate().unwrap();
        let project_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        assert_eq!(workspace.root, PathBuf::from(project_root));
    }

    #[test]
    fn test_workspace_locator_cli() {
        // Equivalent to `pixi xxx` where xxx is any command
        let workspace_locator = WorkspaceLocator::for_cli();
        let workspace = workspace_locator.locate().unwrap();
        let project_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        assert_eq!(workspace.root, PathBuf::from(project_root));
    }

    #[test]
    fn test_workspace_locator_explicit() {
        // Equivalent to `pixi xxx --manifest /absolute/path/to/pixi.toml`
        let project_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let workspace_locator =
            WorkspaceLocator::default().with_search_start(DiscoveryStart::ExplicitManifest(
                Path::new(&project_root).join("pixi.toml").to_path_buf(),
            ));
        let workspace = workspace_locator.locate().unwrap();
        assert_eq!(workspace.root, PathBuf::from(project_root));
    }

    #[test]
    fn test_workspace_locator_explicit_simple() {
        // Equivalent to `pixi xxx --manifest pixi.toml`
        let project_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let workspace_locator = WorkspaceLocator::default().with_search_start(
            DiscoveryStart::ExplicitManifest(Path::new("pixi.toml").to_path_buf()),
        );
        let workspace = workspace_locator.locate().unwrap();
        assert_eq!(workspace.root, PathBuf::from(project_root));
    }

    #[test]
    fn test_workspace_locator_explicit_path() {
        // Equivalent to `pixi xxx --manifest /absolute/path/to/folder`
        let project_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let workspace_locator = WorkspaceLocator::default().with_search_start(
            DiscoveryStart::ExplicitManifest(Path::new(&project_root).to_path_buf()),
        );
        let workspace = workspace_locator.locate().unwrap();
        assert_eq!(workspace.root, PathBuf::from(project_root));
    }
}
