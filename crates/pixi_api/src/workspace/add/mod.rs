use indexmap::IndexMap;
use miette::IntoDiagnostic;
use pixi_core::{
    environment::sanity_check_workspace,
    workspace::{PypiDeps, UpdateDeps, WorkspaceMut},
};
use pixi_manifest::PrioritizedChannel;
use pixi_manifest::{FeatureName, KnownPreviewFeature, SpecType};
use pixi_spec::{GitSpec, SourceLocationSpec, SourceSpec};
use rattler_conda_types::{MatchSpec, NamedChannelOrUrl, PackageName};
mod options;

pub use options::{DependencyOptions, GitOptions};

pub async fn add_conda_dep(
    mut workspace: WorkspaceMut,
    specs: IndexMap<PackageName, MatchSpec>,
    spec_type: SpecType,
    dep_options: DependencyOptions,
    git_options: GitOptions,
    channel: Option<String>,
) -> miette::Result<Option<UpdateDeps>> {
    sanity_check_workspace(workspace.workspace()).await?;

    // Add the platform if it is not already present
    workspace
        .manifest()
        .add_platforms(dep_options.platforms.iter(), &FeatureName::DEFAULT)?;

    let mut match_specs = IndexMap::default();
    let mut source_specs = IndexMap::default();

    // Add a channel if specified

    if channel.is_some() {
        workspace.manifest().add_channels(
            [
                PrioritizedChannel::from(NamedChannelOrUrl::Name(channel.unwrap_or_default()))
                    .clone(),
            ],
            &FeatureName::DEFAULT,
            false,
        )?;
    }

    for (_, spec) in &specs {
        let Some(channel) = spec.channel.as_ref().and_then(|c| c.name.as_ref()) else {
            break;
        };

        workspace.manifest().add_channels(
            [PrioritizedChannel::from(NamedChannelOrUrl::Name(channel.clone())).clone()],
            &FeatureName::DEFAULT,
            false,
        )?;
    }
    // if user passed some git configuration
    // we will use it to create pixi source specs
    let passed_specs: IndexMap<PackageName, (MatchSpec, SpecType)> = specs
        .into_iter()
        .map(|(name, spec)| (name, (spec, spec_type)))
        .collect();

    if let Some(git) = &git_options.git {
        if !workspace
            .manifest()
            .workspace
            .preview()
            .is_enabled(KnownPreviewFeature::PixiBuild)
        {
            return Err(miette::miette!(
                help = format!(
                    "Add `preview = [\"pixi-build\"]` to the `workspace` or `project` table of your manifest ({})",
                    workspace.workspace().workspace.provenance.path.display()
                ),
                "conda source dependencies are not allowed without enabling the 'pixi-build' preview feature"
            ));
        }

        source_specs = passed_specs
            .iter()
            .map(|(name, (_spec, spec_type))| {
                let git_spec = GitSpec {
                    git: git.clone(),
                    rev: Some(git_options.reference.clone()),
                    subdirectory: git_options.subdir.clone(),
                };
                (
                    name.clone(),
                    (
                        SourceSpec {
                            location: SourceLocationSpec::Git(git_spec),
                        },
                        *spec_type,
                    ),
                )
            })
            .collect();
    } else {
        match_specs = passed_specs;
    }

    // TODO: add dry_run logic to add
    let dry_run = false;

    let update_deps = match Box::pin(workspace.update_dependencies(
        match_specs,
        IndexMap::default(),
        source_specs,
        dep_options.no_install,
        &dep_options.lock_file_usage,
        &dep_options.feature,
        &dep_options.platforms,
        false,
        dry_run,
    ))
    .await
    {
        Ok(update_deps) => {
            // Write the updated manifest
            workspace.save().await.into_diagnostic()?;
            update_deps
        }
        Err(e) => {
            workspace.revert().await.into_diagnostic()?;
            return Err(e);
        }
    };

    Ok(update_deps)
}

pub async fn add_pypi_dep(
    mut workspace: WorkspaceMut,
    pypi_deps: PypiDeps,
    editable: bool,
    options: DependencyOptions,
) -> miette::Result<Option<UpdateDeps>> {
    sanity_check_workspace(workspace.workspace()).await?;

    // Add the platform if it is not already present
    workspace
        .manifest()
        .add_platforms(options.platforms.iter(), &FeatureName::DEFAULT)?;

    // TODO: add dry_run logic to add
    let dry_run = false;

    let update_deps = match Box::pin(workspace.update_dependencies(
        IndexMap::default(),
        pypi_deps,
        IndexMap::default(),
        options.no_install,
        &options.lock_file_usage,
        &options.feature,
        &options.platforms,
        editable,
        dry_run,
    ))
    .await
    {
        Ok(update_deps) => {
            // Write the updated manifest
            workspace.save().await.into_diagnostic()?;
            update_deps
        }
        Err(e) => {
            workspace.revert().await.into_diagnostic()?;
            return Err(e);
        }
    };

    Ok(update_deps)
}
