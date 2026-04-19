use std::{path::{Path, PathBuf}, sync::Arc};

use itertools::Itertools;
use miette::{Diagnostic, NamedSource, Report};
use pixi_consts::consts;
use pixi_manifest::{
    ExplicitManifestError, LoadManifestsError, Manifests, TomlError, WarningWithSource,
    WithWarnings, WorkspaceDiscoveryError, utils::WithSourceCode,
};
use thiserror::Error;

use crate::workspace::WorkspaceRegistry;
use crate::workspace::{MemberWorkspace, Workspace, WorkspaceRegistryError};

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

    /// Search by name in the workspace registry.
    ///
    /// If the name is not found in the registry, abort.
    WorkspaceRegistry(String),
}

impl DiscoveryStart {
    /// Returns the path where the search should start.
    pub fn path(&self) -> miette::Result<PathBuf, WorkspaceLocatorError> {
        match self {
            DiscoveryStart::CurrentDir => {
                std::env::current_dir().map_err(WorkspaceLocatorError::CurrentDir)
            }
            DiscoveryStart::SearchRoot(path) => Ok(path.clone()),
            DiscoveryStart::ExplicitManifest(path) => Ok(path.clone()),
            DiscoveryStart::WorkspaceRegistry(name) => {
                let registry = WorkspaceRegistry::load()
                    .map_err(|_| WorkspaceLocatorError::MissingRegistry())?;
                let path = registry.named_workspace(name)?;
                if !path.exists() {
                    return Err(WorkspaceLocatorError::MissingWorkspacePath {
                        name: name.to_string(),
                        path,
                    });
                }
                Ok(path.clone())
            }
        }
    }
}

/// A helper struct that helps discover the workspace root and potentially the
/// "current" package.
#[derive(Default)]
pub struct WorkspaceLocator {
    start: DiscoveryStart,
    with_closest_package: bool,
    emit_warnings: bool,
    consider_environment: bool,
    ignore_pixi_version_check: bool,
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
        "could not find {project_manifest} or {pyproject_manifest} with {pyproject_prefix} at directory {0}",
        project_manifest = consts::WORKSPACE_MANIFEST,
        pyproject_manifest = consts::PYPROJECT_MANIFEST,
        pyproject_prefix = consts::PYPROJECT_PIXI_PREFIX
    )]
    WorkspaceNotFound(PathBuf),

    /// A pyproject.toml file exists but lacks [tool.pixi] configuration.
    #[error(
        "found {pyproject_manifest} without {pyproject_prefix} section at directory {0}\n\nSuggestion: Run 'pixi init' to initialize pixi support in the existing {pyproject_manifest}",
        pyproject_manifest = consts::PYPROJECT_MANIFEST,
        pyproject_prefix = consts::PYPROJECT_PIXI_PREFIX
    )]
    PyprojectWithoutPixi(PathBuf),

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

    #[error("unable to load the workspace registry")]
    MissingRegistry(),

    #[error(transparent)]
    #[diagnostic(transparent)]
    MissingWorkspace(#[from] WorkspaceRegistryError),

    #[error("could not find workspace '{}' at '{}'", .name, .path.display())]
    #[diagnostic(help = "clean the registry with `pixi workspace register prune`")]
    MissingWorkspacePath { name: String, path: PathBuf },

    #[error(transparent)]
    #[diagnostic(transparent)]
    PixiVersionMismatch(#[from] Box<pixi_manifest::PixiVersionMismatchError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    InvalidRequiresPixi(#[from] Box<pixi_manifest::InvalidRequiresPixiError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    MemberDiscovery(#[from] Box<pixi_manifest::MemberDiscoveryError>),
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

    /// When the current version conflicts with the workspace requirement,
    /// whether to generate an error.
    pub fn with_ignore_pixi_version_check(self, ignore_pixi_version_check: bool) -> Self {
        Self {
            ignore_pixi_version_check,
            ..self
        }
    }

    /// Called to locate the workspace or error out if none could be located.
    pub fn locate(self) -> Result<Workspace, WorkspaceLocatorError> {
        // Determine the search root
        let explicit_start = matches!(&self.start, DiscoveryStart::ExplicitManifest(_));
        let discovery_start = match self.start {
            DiscoveryStart::ExplicitManifest(path) => {
                pixi_manifest::DiscoveryStart::ExplicitManifest(path)
            }
            DiscoveryStart::CurrentDir => pixi_manifest::DiscoveryStart::SearchRoot(
                std::env::current_dir().map_err(WorkspaceLocatorError::CurrentDir)?,
            ),
            DiscoveryStart::SearchRoot(path) => pixi_manifest::DiscoveryStart::SearchRoot(path),
            DiscoveryStart::WorkspaceRegistry(name) => {
                let registry = WorkspaceRegistry::load()
                    .map_err(|_| WorkspaceLocatorError::MissingRegistry())?;
                let path = registry
                    .named_workspace(&name)
                    .map_err(WorkspaceLocatorError::MissingWorkspace)?;

                if !path.exists() {
                    return Err(WorkspaceLocatorError::MissingWorkspacePath { name, path });
                }

                pixi_manifest::DiscoveryStart::ExplicitManifest(path)
            }
        };

        let discovery_source = discovery_start.root().to_path_buf();

        // Discover the workspace manifest for the current path.
        let workspace_manifests = match pixi_manifest::WorkspaceDiscoverer::new(discovery_start)
            .with_closest_package(self.with_closest_package)
            .with_ignore_pixi_version_check(self.ignore_pixi_version_check)
            .discover()
        {
            Ok(manifests) => manifests,
            Err(WorkspaceDiscoveryError::Toml(err)) => {
                return Err(WorkspaceLocatorError::Toml(err));
            }
            Err(WorkspaceDiscoveryError::Io(err)) => return Err(WorkspaceLocatorError::Io(err)),
            Err(WorkspaceDiscoveryError::ExplicitManifestError(err)) => {
                return Err(WorkspaceLocatorError::ExplicitManifestError(err));
            }
            Err(WorkspaceDiscoveryError::Canonicalize(source, path)) => {
                return Err(WorkspaceLocatorError::Canonicalize { path, source });
            }
            Err(WorkspaceDiscoveryError::PixiVersionMismatch(err)) => {
                return Err(WorkspaceLocatorError::PixiVersionMismatch(err));
            }
            Err(WorkspaceDiscoveryError::InvalidRequiresPixi(err)) => {
                return Err(WorkspaceLocatorError::InvalidRequiresPixi(err));
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
        if self.consider_environment
            && !explicit_start
            && let Some(WithWarnings {
                value: manifests,
                warnings: mut env_warnings,
            }) =
                Self::apply_environment_overrides(workspace_manifests.take(), self.emit_warnings)?
        {
            warnings.append(&mut env_warnings);
            workspace_manifests = Some(manifests);
        }

        // Early out if discovery failed.
        let Some(discovered_manifests) = workspace_manifests else {
            // Check if a pyproject.toml exists in the discovery source directory
            let pyproject_path = discovery_source.join(consts::PYPROJECT_MANIFEST);
            if pyproject_path.is_file() {
                // Check if it's a valid Python project by looking for project metadata
                if let Ok(content) = fs_err::read_to_string(&pyproject_path)
                    && content.contains("[project]")
                {
                    return Err(WorkspaceLocatorError::PyprojectWithoutPixi(
                        discovery_source,
                    ));
                }
            }
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
                    .format_with("\n", |w, f| f(&format_args!("{w:?}")))
            );
        }

        let mut workspace = Workspace::from_manifests(discovered_manifests);

        // If the `hierarchical-tasks` preview feature is enabled on the
        // discovered root workspace, recursively load each discovered
        // member's own standalone Workspace and attach it to the root's
        // member tree. Under Model 2 every member has its own [workspace]
        // and is fully self-contained — we just load it via the same
        // locator machinery, then key the results by member name.
        if workspace
            .workspace
            .value
            .workspace
            .preview
            .is_enabled(pixi_manifest::KnownPreviewFeature::HierarchicalTasks)
        {
            // Find the MemberTree that was populated during manifest
            // discovery and load each member. This runs the full
            // `WorkspaceLocator` for each member directory, which means
            // each member gets the same discovery/canonicalisation/
            // requires-pixi treatment as any standalone workspace.
            let workspace_dir = workspace.root().to_path_buf();
            let members = load_members_recursively(&workspace_dir)?;
            workspace.set_members(members);
        }

        Ok(workspace)
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
            if let Some(env_manifest_path) = env_manifest_path
                && &env_manifest_path != discovered_manifest_path
                && in_shell
                && emit_warnings
            {
                tracing::warn!(
                    "Using local manifest {} rather than {} from environment variable `PIXI_PROJECT_MANIFEST`",
                    discovered_manifest_path.display(),
                    env_manifest_path.display(),
                );
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
                    ));
                }
            }
        }

        Ok(discovered_workspace.map(WithWarnings::from))
    }
}

/// Recursively load each discovered member as its own standalone
/// [`Workspace`], keyed by the member's `[workspace].name`.
///
/// This is called by [`WorkspaceLocator::locate`] once the root workspace
/// has been constructed and the `hierarchical-tasks` preview feature is
/// enabled. Each member goes through the same [`WorkspaceLocator`] path as
/// a standalone workspace — so the member's own manifest discovery,
/// canonicalisation, and `requires-pixi` check all run unchanged.
///
/// The `workspace_dir` argument is the already-canonicalised root of the
/// outer workspace. Member discovery only descends from here; it never
/// climbs back upward, so a member's own upward walk reliably stops at
/// the member's `[workspace]` (and not the outer root).
fn load_members_recursively(
    workspace_dir: &Path,
) -> Result<indexmap::IndexMap<String, MemberWorkspace>, WorkspaceLocatorError> {
    // Structural discovery: names + directories only, no Workspace load yet.
    let tree = pixi_manifest::discover_members(workspace_dir)
        .map_err(Box::new)
        .map_err(WorkspaceLocatorError::MemberDiscovery)?;

    fn build(
        nodes: &indexmap::IndexMap<String, pixi_manifest::MemberNode>,
    ) -> Result<indexmap::IndexMap<String, MemberWorkspace>, WorkspaceLocatorError> {
        let mut out = indexmap::IndexMap::with_capacity(nodes.len());
        for (name, node) in nodes {
            // Load this member as its own standalone Workspace. We
            // deliberately avoid `with_consider_environment` / warning
            // emission so member loads don't pollute the root's output.
            let member_ws = WorkspaceLocator::for_cli()
                .with_consider_environment(false)
                .with_emit_warnings(false)
                .with_search_start(DiscoveryStart::SearchRoot(node.dir.clone()))
                .locate()?;

            // Recurse into this member's own nested members. Each level
            // owns its own loaded Workspace; the tree mirrors the
            // structure returned by `discover_members`.
            let children = build(&node.children)?;

            out.insert(
                name.clone(),
                MemberWorkspace {
                    workspace: member_ws,
                    children,
                },
            );
        }
        Ok(out)
    }

    build(tree.members())
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use temp_env;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_workspace_locator() {
        let workspace_locator = WorkspaceLocator::default();
        let workspace = workspace_locator.locate().unwrap();
        let crate_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let project_root = Path::new(&crate_root).parent().unwrap().parent().unwrap();
        assert_eq!(workspace.root, project_root);
    }

    #[test]
    fn test_workspace_locator_cli() {
        // Equivalent to `pixi xxx` where xxx is any command
        let workspace_locator = WorkspaceLocator::for_cli();
        let workspace = workspace_locator.locate().unwrap();
        let crate_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let project_root = Path::new(&crate_root).parent().unwrap().parent().unwrap();
        assert_eq!(workspace.root, project_root);
    }

    #[test]
    fn test_workspace_locator_explicit() {
        // Equivalent to `pixi xxx --manifest /absolute/path/to/pixi.toml`
        let crate_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let project_root = Path::new(&crate_root).parent().unwrap().parent().unwrap();
        let workspace_locator = WorkspaceLocator::default().with_search_start(
            DiscoveryStart::ExplicitManifest(project_root.join("pixi.toml").to_path_buf()),
        );
        let workspace = workspace_locator.locate().unwrap();
        assert_eq!(workspace.root, project_root);
    }

    #[tokio::test]
    async fn test_workspace_locator_registered_workspace() {
        // Equivalent to `pixi xxx --workspace ws`
        let temp_dir = tempdir().unwrap();
        let pixi_home_dir = temp_dir.path().join("pixi-home");
        let crate_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let project_root = Path::new(&crate_root).parent().unwrap().parent().unwrap();
        temp_env::async_with_vars(
            [("PIXI_HOME", Some(pixi_home_dir.to_str().unwrap()))],
            async {
                WorkspaceRegistry::default()
                    .add_workspace("ws".to_string(), project_root.to_path_buf())
                    .await
                    .unwrap();

                let workspace_locator = WorkspaceLocator::default()
                    .with_search_start(DiscoveryStart::WorkspaceRegistry("ws".to_string()));
                let workspace = workspace_locator.locate().unwrap();
                assert_eq!(workspace.root, project_root);
            },
        )
        .await;
    }

    #[tokio::test]
    async fn test_workspace_locator_registered_workspace_path_does_not_exist() {
        // Equivalent to `pixi xxx --workspace ws`
        let temp_dir = tempdir().unwrap();
        let pixi_home_dir = temp_dir.path().join("pixi-home");
        let crate_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let invalid_project_root = Path::new(&crate_root)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("idontexist");
        temp_env::async_with_vars(
            [("PIXI_HOME", Some(pixi_home_dir.to_str().unwrap()))],
            async {
                WorkspaceRegistry::default()
                    .add_workspace("ws".to_string(), invalid_project_root.to_path_buf())
                    .await
                    .unwrap();

                let workspace_locator = WorkspaceLocator::default()
                    .with_search_start(DiscoveryStart::WorkspaceRegistry("ws".to_string()));
                let result = workspace_locator.locate();
                assert!(result.is_err());

                let error = result.unwrap_err();
                assert!(matches!(
                    error,
                    WorkspaceLocatorError::MissingWorkspacePath { name: _, path: _ }
                ));

                // Check that the error message contains the suggestion
                let error_message = error.to_string();
                assert!(error_message.contains("could not find workspace "));
                assert!(error_message.contains("ws"));
            },
        )
        .await;
    }

    #[test]
    fn test_workspace_locator_registered_workspace_does_not_exist() {
        // Equivalent to `pixi xxx --workspace idontexist`
        let temp_dir = tempdir().unwrap();
        let pixi_home_dir = temp_dir.path().join("pixi-home");
        temp_env::with_var("PIXI_HOME", Some(pixi_home_dir.to_str().unwrap()), || {
            let workspace_locator = WorkspaceLocator::default()
                .with_search_start(DiscoveryStart::WorkspaceRegistry("idontexist".to_string()));

            let result = workspace_locator.locate();
            assert!(result.is_err());

            let error = result.unwrap_err();
            assert!(matches!(error, WorkspaceLocatorError::MissingWorkspace(_)));

            // Check that the error message contains the suggestion
            let error_message = error.to_string();
            assert!(error_message.contains("could not find workspace "));
            assert!(error_message.contains("idontexist"));
        });
    }

    #[test]
    fn test_workspace_locator_explicit_simple() {
        // Equivalent to `pixi xxx --manifest pixi.toml`
        let crate_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let project_root = Path::new(&crate_root).parent().unwrap().parent().unwrap();
        let workspace_locator = WorkspaceLocator::default().with_search_start(
            DiscoveryStart::ExplicitManifest(Path::new("../../pixi.toml").to_path_buf()),
        );
        let workspace = workspace_locator.locate().unwrap();
        assert_eq!(workspace.root, project_root);
    }

    #[test]
    fn test_workspace_locator_explicit_path() {
        // Equivalent to `pixi xxx --manifest /absolute/path/to/folder`
        let crate_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let project_root = Path::new(&crate_root).parent().unwrap().parent().unwrap();
        let workspace_locator = WorkspaceLocator::default()
            .with_search_start(DiscoveryStart::ExplicitManifest(project_root.to_path_buf()));
        let workspace = workspace_locator.locate().unwrap();
        assert_eq!(workspace.root, project_root);
    }

    #[test]
    fn test_pyproject_without_pixi_error() {
        use tempfile::TempDir;

        // Create a temporary directory
        let temp_dir = TempDir::new().unwrap();
        let temp_path = temp_dir.path();

        // Create a pyproject.toml file without [tool.pixi] section
        let pyproject_content = r#"
[project]
name = "test-project"
version = "0.1.0"
description = "A test project"
dependencies = []
"#;
        let pyproject_path = temp_path.join("pyproject.toml");
        fs_err::write(&pyproject_path, pyproject_content).unwrap();

        // Try to locate workspace - should return PyprojectWithoutPixi error
        let workspace_locator = WorkspaceLocator::default()
            .with_search_start(DiscoveryStart::SearchRoot(temp_path.to_path_buf()));

        let result = workspace_locator.locate();
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(matches!(
            error,
            WorkspaceLocatorError::PyprojectWithoutPixi(_)
        ));

        // Check that the error message contains the suggestion
        let error_message = error.to_string();
        assert!(error_message.contains("pixi init"));
        assert!(error_message.contains("pyproject.toml"));
        assert!(error_message.contains("tool.pixi"));
    }
}
