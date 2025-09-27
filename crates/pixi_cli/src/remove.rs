use clap::Parser;
use miette::{Context, IntoDiagnostic};
use pixi_config::ConfigCli;
use pixi_core::{
    DependencyType, UpdateLockFileOptions, WorkspaceLocator,
    environment::{InstallFilter, get_update_lock_file_and_prefix},
    lock_file::{ReinstallPackages, UpdateMode},
};
use pixi_manifest::FeaturesExt;

use crate::cli_config::{DependencyConfig, NoInstallConfig, WorkspaceConfig};
use crate::{cli_config::LockFileUpdateConfig, has_specs::HasSpecs};

/// Removes dependencies from the workspace.
///
///  If the workspace manifest is a `pyproject.toml`, removing a pypi dependency
/// with the `--pypi` flag will remove it from either
///
/// - the native pyproject `project.dependencies` array or, if a feature is
///   specified, the native `project.optional-dependencies` table
///
/// - pixi `pypi-dependencies` tables of the default feature or, if a feature is
///   specified, a named feature
#[derive(Debug, Default, Parser)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    pub dependency_config: DependencyConfig,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,
    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    pub config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let (dependency_config, no_install_config, lock_file_update_config, workspace_config) = (
        args.dependency_config,
        args.no_install_config,
        args.lock_file_update_config,
        args.workspace_config,
    );

    let mut workspace = WorkspaceLocator::for_cli()
        .with_search_start(workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(args.config.clone())
        .modify()?;
    let dependency_type = dependency_config.dependency_type();

    // Prevent removing Python if PyPI dependencies exist
    if let DependencyType::CondaDependency(_) = dependency_type {
        for name in dependency_config.specs()?.keys() {
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
    }
    match dependency_type {
        DependencyType::PypiDependency => {
            for name in dependency_config.pypi_deps(workspace.workspace())?.keys() {
                workspace
                    .manifest()
                    .remove_pypi_dependency(
                        name,
                        &dependency_config.platforms,
                        &dependency_config.feature,
                    )
                    .wrap_err(format!(
                        "failed to remove PyPI dependency: '{}'",
                        name.as_source()
                    ))?;
            }
        }
        DependencyType::CondaDependency(spec_type) => {
            for name in dependency_config.specs()?.keys() {
                workspace
                    .manifest()
                    .remove_dependency(
                        name,
                        spec_type,
                        &dependency_config.platforms,
                        &dependency_config.feature,
                    )
                    .wrap_err(format!(
                        "failed to remove dependency: '{}'",
                        name.as_source()
                    ))?;
            }
        }
    };

    let workspace = workspace.save().await.into_diagnostic()?;

    // TODO: update all environments touched by this feature defined.
    // updating prefix after removing from toml
    if !lock_file_update_config.no_lockfile_update {
        get_update_lock_file_and_prefix(
            &workspace.default_environment(),
            UpdateMode::Revalidate,
            UpdateLockFileOptions {
                lock_file_usage: lock_file_update_config.lock_file_usage()?,
                no_install: no_install_config.no_install,
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
            },
            ReinstallPackages::default(),
            &InstallFilter::default(),
        )
        .await?;
    }

    dependency_config.display_success("Removed", Default::default());

    Ok(())
}
