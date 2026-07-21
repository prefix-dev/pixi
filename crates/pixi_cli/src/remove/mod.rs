mod error;

use std::path::PathBuf;

use clap::Parser;
use indexmap::IndexMap;
use pixi_api::{
    WorkspaceContext,
    workspace::{DependencyOptions, RemoveError},
};
use pixi_config::ConfigCli;
use pixi_core::{DependencyType, WorkspaceLocator};
use pixi_manifest::HasWorkspaceManifest;

use crate::{cli_config::LockFileUpdateConfig, has_specs::HasSpecs};
use crate::{
    cli_config::{DependencyConfig, NoInstallConfig, WorkspaceConfig},
    cli_interface::CliInterface,
};

use error::DependencyRemovalError;

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
// `pixi script` builds these Args with `..Default::default()`, so a clap
// `default_value` on any field must match the field's Rust default.
#[derive(Debug, Default, Parser)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// Internal script path supplied by `pixi script remove`.
    #[arg(skip)]
    #[doc(hidden)]
    pub script: Option<PathBuf>,

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
    args.dependency_config.warn_deprecated_subdir();

    let workspace = if let Some(path) = &args.script {
        let script = crate::script::require_script(path)?;
        crate::script::warn_portability(&script)?;
        crate::script::script_workspace(
            script,
            &args.config_source.source(),
            args.config.clone().into(),
            None,
        )?
    } else {
        WorkspaceLocator::for_cli()
            .with_global_config_source(args.config_source.source())
            .with_search_start(args.workspace_config.workspace_locator_start())
            .locate()?
            .with_cli_config(args.config.clone())
    };

    let requested_lock_file_usage = args.lock_file_update_config.lock_file_usage()?;
    let dependency_options = DependencyOptions {
        feature: args.dependency_config.feature_name(),
        platforms: args.dependency_config.platforms.clone(),
        no_install: args.no_install_config.no_install,
        lock_file_usage: if args.script.is_some() {
            crate::script::edit_lock_file_usage(
                requested_lock_file_usage,
                workspace.lock_file_path().is_file(),
            )?
        } else {
            requested_lock_file_usage
        },
    };

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace.clone());

    let dependency_type = args.dependency_config.dependency_type();
    let feature = args.dependency_config.feature_name();
    let platforms = args.dependency_config.platforms.clone();

    let result = match dependency_type {
        DependencyType::CondaDependency(spec_type) => {
            let specs = args.dependency_config.specs()?;
            let names: Vec<String> = specs
                .keys()
                .map(|n| n.as_normalized().to_string())
                .collect();
            (
                workspace_ctx
                    .remove_conda_deps(specs, spec_type, dependency_options)
                    .await,
                names,
            )
        }
        DependencyType::PypiDependency => {
            let pypi_deps = args.dependency_config.pypi_deps(&workspace)?;
            let names: Vec<String> = pypi_deps
                .keys()
                .map(|n| n.as_source().to_string())
                .collect();
            let pypi_deps: IndexMap<_, _> = pypi_deps
                .into_iter()
                .map(|(name, req)| (name, (req, None, None)))
                .collect();
            (
                workspace_ctx
                    .remove_pypi_deps(pypi_deps, dependency_options)
                    .await,
                names,
            )
        }
    };

    match result {
        (Ok(()), _) => {
            args.dependency_config
                .display_success("Removed", Default::default());
            Ok(())
        }
        (Err(RemoveError::NotFound { name: missing }), typed_names) => {
            // Show the spelling the user typed, not the manifest's normalized form.
            let name = typed_names
                .iter()
                .find(|n| n.eq_ignore_ascii_case(&missing))
                .cloned()
                .unwrap_or(missing);
            Err(miette::Report::new(DependencyRemovalError::new(
                name,
                (&workspace).workspace_manifest(),
                dependency_type,
                &feature,
                &platforms,
            )))
        }
        (Err(other), _) => Err(miette::Report::new(other)),
    }
}
