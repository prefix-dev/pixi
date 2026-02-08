use indexmap::IndexMap;
use miette::{Context, IntoDiagnostic};
use pixi_core::{
    InstallFilter, UpdateLockFileOptions,
    environment::{LockFileUsage, get_update_lock_file_and_prefix},
    lock_file::{ReinstallPackages, UpdateMode},
    workspace::{PypiDeps, WorkspaceMut},
};
use pixi_manifest::{FeaturesExt, SpecType};
use rattler_conda_types::{MatchSpec, PackageName};

use crate::workspace::DependencyOptions;

pub async fn remove_conda_deps(
    mut workspace: WorkspaceMut,
    specs: IndexMap<PackageName, MatchSpec>,
    spec_type: SpecType,
    options: DependencyOptions,
) -> miette::Result<()> {
    // Prevent removing Python if PyPI dependencies exist
    for name in specs.keys() {
        if name.as_source() == "python" {
            // Check if there are any PyPI dependencies by importing the PypiDependencies trait
            let pypi_deps = workspace
                .workspace()
                .default_environment()
                .pypi_dependencies(None);
            if !pypi_deps.is_empty() {
                let deps_list = pypi_deps
                    .iter()
                    .map(|(name, _)| name.as_source())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(miette::miette!(
                    "Cannot remove Python while PyPI dependencies exist. Please remove these PyPI dependencies first: {}",
                    deps_list
                ));
            }
        }
    }

    for name in specs.keys() {
        workspace
            .manifest()
            .remove_dependency(name, spec_type, &options.platforms, &options.feature)
            .wrap_err(format!(
                "failed to remove dependency: '{}'",
                name.as_source()
            ))?;
    }
    let workspace = workspace.save().await.into_diagnostic()?;

    // TODO: update all environments touched by this feature defined.
    // updating prefix after removing from toml
    if options.lock_file_usage == LockFileUsage::Update {
        get_update_lock_file_and_prefix(
            &workspace.default_environment(),
            UpdateMode::Revalidate,
            UpdateLockFileOptions {
                lock_file_usage: options.lock_file_usage,
                no_install: options.no_install,
                pypi_no_deps: false,
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
            },
            ReinstallPackages::default(),
            &InstallFilter::default(),
        )
        .await?;
    }

    Ok(())
}

pub async fn remove_pypi_deps(
    mut workspace: WorkspaceMut,
    pypi_deps: PypiDeps,
    options: DependencyOptions,
) -> miette::Result<()> {
    for name in pypi_deps.keys() {
        workspace
            .manifest()
            .remove_pypi_dependency(name, &options.platforms, &options.feature)
            .wrap_err(format!(
                "failed to remove PyPI dependency: '{}'",
                name.as_source()
            ))?;
    }

    let workspace = workspace.save().await.into_diagnostic()?;

    // TODO: update all environments touched by this feature defined.
    // updating prefix after removing from toml
    if options.lock_file_usage == LockFileUsage::Update {
        get_update_lock_file_and_prefix(
            &workspace.default_environment(),
            UpdateMode::Revalidate,
            UpdateLockFileOptions {
                lock_file_usage: options.lock_file_usage,
                no_install: options.no_install,
                pypi_no_deps: false,
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
            },
            ReinstallPackages::default(),
            &InstallFilter::default(),
        )
        .await?;
    }

    Ok(())
}
