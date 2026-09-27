use std::path::{Path, PathBuf};

use miette::Diagnostic;
use pixi_manifest::{FeaturesExt, HasWorkspaceManifest};
use pixi_pypi_spec::PixiPypiSource;
use pixi_spec::{PixiSpec, SourceLocationSpec};
use thiserror::Error;

use crate::Workspace;
use crate::workspace::Environment;

/// One dependency that would require a build backend under `--no-build`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoBuildRefusal {
    pub name: String,
    pub reason: NoBuildReason,
}

/// Why a dependency cannot be resolved without executing a build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoBuildReason {
    Git,
    Path,
    Url,
    WorkspacePackage,
    Sdist,
}

impl NoBuildReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Path => "path",
            Self::Url => "url",
            Self::WorkspacePackage => "workspace package",
            Self::Sdist => "sdist",
        }
    }
}

impl std::fmt::Display for NoBuildRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.reason {
            NoBuildReason::WorkspacePackage => {
                write!(f, "workspace package `{}`", self.name)
            }
            other => write!(f, "source dependency `{}` ({})", self.name, other.as_str()),
        }
    }
}

/// Fail-closed error for `pixi lock` / `update` / `upgrade --no-build`.
#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
#[diagnostic(help("drop --no-build to resolve source dependencies"))]
pub struct NoBuildRefused {
    message: String,
    pub refusals: Vec<NoBuildRefusal>,
}

impl NoBuildRefused {
    fn from_refusals(refusals: Vec<NoBuildRefusal>) -> Self {
        let message = if refusals.len() == 1 {
            format!("refusing to invoke a build backend for {}", refusals[0])
        } else {
            let lines = refusals
                .iter()
                .map(|refusal| format!("- {refusal}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("refusing to invoke a build backend while updating the lock file:\n{lines}")
        };
        Self { message, refusals }
    }
}

/// Scan every environment this command will verify and refuse source specs
/// that would invoke a build backend. Channel packages, conda binary
/// archives, and PyPI wheels are not refusals.
pub fn refuse_if_would_execute(workspace: &Workspace) -> miette::Result<()> {
    let mut refusals = Vec::new();
    for environment in workspace.environments() {
        collect_environment_refusals(workspace, &environment, &mut refusals);
    }
    if refusals.is_empty() {
        Ok(())
    } else {
        Err(NoBuildRefused::from_refusals(refusals).into())
    }
}

fn collect_environment_refusals(
    workspace: &Workspace,
    environment: &Environment<'_>,
    refusals: &mut Vec<NoBuildRefusal>,
) {
    let mut platforms = vec![None];
    let manifest = environment.workspace_manifest();
    let env_platform_names = environment.platforms();
    for platform in manifest
        .workspace
        .platforms
        .iter()
        .filter(|platform| env_platform_names.contains(platform.name()))
    {
        platforms.push(Some(platform));
    }

    for platform in platforms {
        for (name, specs) in environment.combined_dependencies(platform).iter() {
            for spec in specs {
                if let Some(refusal) = conda_spec_refusal(workspace, name.as_source(), spec) {
                    push_unique(refusals, refusal);
                }
            }
        }
        for (name, specs) in environment.pypi_dependencies(platform).iter() {
            for spec in specs {
                if let Some(refusal) = pypi_source_refusal(name.as_source(), spec.source()) {
                    push_unique(refusals, refusal);
                }
            }
        }
        for (name, specs) in environment.combined_dev_dependencies(platform).iter() {
            for spec in specs {
                if let Some(refusal) = dev_spec_refusal(workspace, name.as_source(), spec) {
                    push_unique(refusals, refusal);
                }
            }
        }
    }
}

fn push_unique(refusals: &mut Vec<NoBuildRefusal>, refusal: NoBuildRefusal) {
    if !refusals
        .iter()
        .any(|existing| existing.name == refusal.name && existing.reason == refusal.reason)
    {
        refusals.push(refusal);
    }
}

fn conda_spec_refusal(
    workspace: &Workspace,
    name: &str,
    spec: &PixiSpec,
) -> Option<NoBuildRefusal> {
    if !spec.is_source() {
        return None;
    }
    let reason = match spec {
        PixiSpec::Git(_) => NoBuildReason::Git,
        PixiSpec::UrlSource(_) => NoBuildReason::Url,
        PixiSpec::PathSource(path) => {
            if is_workspace_package_path(workspace, Path::new(path.path.as_str())) {
                NoBuildReason::WorkspacePackage
            } else {
                NoBuildReason::Path
            }
        }
        _ => return None,
    };
    Some(NoBuildRefusal {
        name: name.to_string(),
        reason,
    })
}

fn dev_spec_refusal(
    workspace: &Workspace,
    name: &str,
    spec: &SourceLocationSpec,
) -> Option<NoBuildRefusal> {
    let reason = match spec {
        SourceLocationSpec::Git(_) => NoBuildReason::Git,
        SourceLocationSpec::Url(url) if url.is_binary() => return None,
        SourceLocationSpec::Url(_) => NoBuildReason::Url,
        SourceLocationSpec::Path(path) if path.is_binary() => return None,
        SourceLocationSpec::Path(path) => {
            if is_workspace_package_path(workspace, Path::new(path.path.as_str())) {
                NoBuildReason::WorkspacePackage
            } else {
                NoBuildReason::Path
            }
        }
    };
    Some(NoBuildRefusal {
        name: name.to_string(),
        reason,
    })
}

fn pypi_source_refusal(name: &str, source: &PixiPypiSource) -> Option<NoBuildRefusal> {
    if !source.is_source_dependency() {
        return None;
    }
    let reason = match source {
        PixiPypiSource::Git { .. } => NoBuildReason::Git,
        PixiPypiSource::Path { path, .. } => {
            if is_wheel_path(path.inner()) {
                return None;
            }
            NoBuildReason::Path
        }
        PixiPypiSource::Url { url, .. } => {
            if is_wheel_url(url.as_str()) {
                return None;
            }
            NoBuildReason::Url
        }
        PixiPypiSource::Registry { .. } => return None,
    };
    Some(NoBuildRefusal {
        name: name.to_string(),
        reason,
    })
}

fn is_wheel_path(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("whl")
}

fn is_wheel_url(url: &str) -> bool {
    let path = url.split('?').next().unwrap_or(url);
    let path = path.split('#').next().unwrap_or(path);
    path.ends_with(".whl")
}

fn is_workspace_package_path(workspace: &Workspace, spec_path: &Path) -> bool {
    let Some(package) = workspace.package.as_ref() else {
        return false;
    };
    let Some(package_dir) = package.provenance.path.parent() else {
        return false;
    };
    paths_match(workspace.root(), spec_path, package_dir)
}

fn paths_match(root: &Path, spec_path: &Path, package_dir: &Path) -> bool {
    let resolved = resolve_against(root, spec_path);
    let package_dir = normalize(package_dir);
    normalize(&resolved) == package_dir
}

fn resolve_against(root: &Path, spec_path: &Path) -> PathBuf {
    if spec_path.is_absolute() {
        spec_path.to_path_buf()
    } else {
        root.join(spec_path)
    }
}

fn normalize(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
