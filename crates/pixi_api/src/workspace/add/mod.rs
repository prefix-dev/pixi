use std::path::PathBuf;

use futures::TryFutureExt;
use indexmap::IndexMap;
use miette::Diagnostic;
use pixi_core::{
    Workspace,
    environment::{SanityCheckError, sanity_check_workspace},
    workspace::{PypiDeps, SkippedPackage, UpdateDependenciesError, UpdateDeps, WorkspaceMut},
};
use pixi_manifest::{
    DependencyOverwriteBehavior, FeatureName, HasWorkspaceManifest, KnownPreviewFlag,
    LoadManifestsError, PlatformEditError, SpecType,
};
use pixi_spec::{GitSpec, PathSourceSpec, SourceSpec, Subdirectory, SubdirectoryError};
use rattler_conda_types::{MatchSpec, PackageName};
use thiserror::Error;

use crate::workspace::{
    platforms::resolve_platforms,
    workspace::{SaveManifestError, save},
};

mod options;

pub use options::{DependencyOptions, GitOptions};

#[derive(Debug, Error, Diagnostic)]
pub enum AddError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    LoadWorkspace(#[from] LoadManifestsError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SanityCheck(#[from] SanityCheckError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Platform(#[from] PlatformEditError),

    #[error(
        "conda source dependencies are not allowed without enabling the 'pixi-build' preview flag"
    )]
    #[diagnostic(help("Run `pixi workspace preview add pixi-build` to enable the preview flag"))]
    PixiBuildNotEnabled,

    #[error(transparent)]
    Subdirectory(#[from] SubdirectoryError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    UpdateDependencies(#[from] UpdateDependenciesError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Save(#[from] SaveManifestError),

    #[error("failed to restore '{}'", .path.display())]
    RestoreManifest {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub async fn add_conda_dep(
    mut workspace: WorkspaceMut,
    specs: IndexMap<PackageName, MatchSpec>,
    spec_type: SpecType,
    dep_options: DependencyOptions,
    git_options: GitOptions,
) -> Result<(Option<UpdateDeps>, Vec<SkippedPackage>), AddError> {
    sanity_check_workspace(workspace.workspace()).await?;

    // Resolve the requested platforms, accepting bare subdirs as subdir
    // platforms, and add any that the workspace does not yet declare.
    let workspace_platforms = workspace
        .workspace()
        .workspace_manifest()
        .workspace
        .platforms
        .clone();
    let pixi_platforms = resolve_platforms(&workspace_platforms, &dep_options.platforms)?;
    workspace
        .manifest()
        .add_platforms(pixi_platforms.iter(), &FeatureName::Default)?;

    let mut match_specs = IndexMap::default();
    let mut source_specs = IndexMap::default();

    // if user passed some git configuration
    // we will use it to create pixi source specs
    let passed_specs: IndexMap<PackageName, (MatchSpec, SpecType)> = specs
        .into_iter()
        .map(|(name, spec)| (name, (spec, spec_type)))
        .collect();

    if git_options.git.is_some() || git_options.path.is_some() {
        if !workspace
            .manifest()
            .workspace
            .preview()
            .is_enabled(KnownPreviewFlag::PixiBuild)
        {
            return Err(AddError::PixiBuildNotEnabled);
        }

        if let Some(git) = &git_options.git {
            let subdirectory = git_options
                .subdir
                .clone()
                .map(Subdirectory::try_from)
                .transpose()?
                .unwrap_or_default();
            source_specs = passed_specs
                .iter()
                .map(|(name, (_spec, spec_type))| {
                    let git_spec = GitSpec::new(
                        git.clone(),
                        Some(git_options.reference.clone()),
                        subdirectory.clone(),
                    );
                    (name.clone(), (SourceSpec::from(git_spec), *spec_type))
                })
                .collect();
        } else if let Some(path) = &git_options.path {
            source_specs = passed_specs
                .iter()
                .map(|(name, (_spec, spec_type))| {
                    let path_spec = PathSourceSpec::new(manifest_path_string(path));
                    (name.clone(), (SourceSpec::from(path_spec), *spec_type))
                })
                .collect();
        }
    } else {
        match_specs = passed_specs;
    }

    // TODO: add dry_run logic to add
    let dry_run = false;

    let targets = workspace.target_selectors_for_platforms(&dep_options.platforms);
    let (update_deps, skipped) = match Box::pin(workspace.update_dependencies(
        match_specs,
        IndexMap::default(),
        source_specs,
        dep_options.no_install,
        &dep_options.lock_file_usage,
        &dep_options.feature,
        &targets,
        false,
        dry_run,
        DependencyOverwriteBehavior::OverwriteIfExplicit,
    ))
    .await
    {
        Ok(result) => {
            // Write the updated manifest
            save(workspace).await?;
            result
        }
        Err(e) => {
            revert(workspace).await?;
            return Err(e.into());
        }
    };

    Ok((update_deps, skipped))
}

/// Reverts `workspace`, naming the manifest in the error since the io error
/// doesn't always carry the path.
///
/// Not `async`, for the same reason as [`save`].
fn revert(workspace: WorkspaceMut) -> impl Future<Output = Result<Workspace, AddError>> {
    let path = workspace.workspace().workspace.provenance.path.clone();
    workspace
        .revert()
        .map_err(move |source| AddError::RestoreManifest { path, source })
}

fn manifest_path_string(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub async fn add_pypi_dep(
    mut workspace: WorkspaceMut,
    pypi_deps: PypiDeps,
    editable: bool,
    options: DependencyOptions,
) -> Result<(Option<UpdateDeps>, Vec<SkippedPackage>), AddError> {
    sanity_check_workspace(workspace.workspace()).await?;

    // Resolve the requested platforms, accepting bare subdirs as subdir
    // platforms, and add any that the workspace does not yet declare.
    let workspace_platforms = workspace
        .workspace()
        .workspace_manifest()
        .workspace
        .platforms
        .clone();
    let pixi_platforms = resolve_platforms(&workspace_platforms, &options.platforms)?;
    workspace
        .manifest()
        .add_platforms(pixi_platforms.iter(), &FeatureName::Default)?;

    // TODO: add dry_run logic to add
    let dry_run = false;

    let targets = workspace.target_selectors_for_platforms(&options.platforms);
    let (update_deps, skipped) = match Box::pin(workspace.update_dependencies(
        IndexMap::default(),
        pypi_deps,
        IndexMap::default(),
        options.no_install,
        &options.lock_file_usage,
        &options.feature,
        &targets,
        editable,
        dry_run,
        DependencyOverwriteBehavior::OverwriteIfExplicit,
    ))
    .await
    {
        Ok(result) => {
            // Write the updated manifest
            save(workspace).await?;
            result
        }
        Err(e) => {
            revert(workspace).await?;
            return Err(e.into());
        }
    };

    Ok((update_deps, skipped))
}

#[cfg(test)]
mod tests {
    use pixi_core::environment::LockFileUsage;
    use pixi_manifest::PixiPlatformName;
    use pixi_spec::GitReference;
    use rattler_conda_types::{ParseMatchSpecOptions, RepodataRevision};

    use super::*;

    fn workspace_mut() -> WorkspaceMut {
        // The workspace re-reads its manifest from disk during `modify()`, so
        // we write the TOML to a temp dir and load from that path. The dir is
        // intentionally leaked for the duration of the test.
        let tmp = tempfile::TempDir::new().unwrap().keep();
        let path = tmp.join("pixi.toml");
        fs_err::write(
            &path,
            r#"
[workspace]
name = "test"
channels = []
platforms = ["linux-64"]
"#,
        )
        .unwrap();
        Workspace::from_path(&path)
            .expect("failed to load workspace")
            .modify()
            .unwrap()
    }

    fn options(platforms: &[&str]) -> DependencyOptions {
        DependencyOptions {
            feature: FeatureName::Default,
            platforms: platforms
                .iter()
                .map(|p| PixiPlatformName::try_from(*p).unwrap())
                .collect(),
            no_install: true,
            lock_file_usage: LockFileUsage::Frozen,
        }
    }

    fn git_options(path: Option<&str>) -> GitOptions {
        GitOptions {
            git: None,
            path: path.map(PathBuf::from),
            reference: GitReference::DefaultBranch,
            subdir: None,
        }
    }

    fn specs(name: &str) -> IndexMap<PackageName, MatchSpec> {
        let spec = MatchSpec::from_str(
            name,
            ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
        )
        .unwrap();
        IndexMap::from([(spec.name.as_exact().unwrap().clone(), spec)])
    }

    #[tokio::test]
    async fn unknown_platform() {
        let err = add_conda_dep(
            workspace_mut(),
            specs("ruff"),
            SpecType::Run,
            options(&["nope"]),
            git_options(None),
        )
        .await
        .err()
        .expect("add should fail");
        assert!(matches!(
            err,
            AddError::Platform(PlatformEditError::NotFound(name)) if name.as_str() == "nope"
        ));
    }

    #[tokio::test]
    async fn source_dependency_needs_pixi_build() {
        let err = add_conda_dep(
            workspace_mut(),
            specs("foo"),
            SpecType::Run,
            options(&[]),
            git_options(Some("foo")),
        )
        .await
        .err()
        .expect("add should fail");
        assert!(matches!(err, AddError::PixiBuildNotEnabled));
    }
}
