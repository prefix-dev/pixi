use clap::Parser;
use pixi_api::{WorkspaceContext, workspace::DependencyOptions};
use pixi_config::ConfigCli;
use pixi_core::{DependencyType, WorkspaceLocator};
use pixi_manifest::FeaturesExt;

use crate::{cli_config::LockFileUpdateConfig, has_specs::HasSpecs};
use crate::{
    cli_config::{DependencyConfig, NoInstallConfig, WorkspaceConfig},
    cli_interface::CliInterface,
};

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

impl TryFrom<&Args> for DependencyOptions {
    type Error = miette::Error;

    fn try_from(args: &Args) -> miette::Result<Self> {
        Ok(DependencyOptions {
            feature: args.dependency_config.feature.clone(),
            platforms: args.dependency_config.platforms.clone(),
            no_install: args.no_install_config.no_install,
            lock_file_usage: args.lock_file_update_config.lock_file_usage()?,
        })
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(args.config.clone());

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace.clone());

    let mut dep_type = args.dependency_config.dependency_type();

    // If the user didn't explicitly pass --pypi and the dependency type is conda,
    // check whether the packages actually exist as conda deps. If not, fall back
    // to pypi removal when the packages exist there instead.
    if let DependencyType::CondaDependency(spec_type) = dep_type {
        let specs = args.dependency_config.specs()?;
        let env = workspace.default_environment();
        let conda_deps = env.dependencies(spec_type, None);

        let all_missing_from_conda = specs
            .keys()
            .all(|name| !conda_deps.contains_key(name));

        if all_missing_from_conda {
            let pypi_deps = env.pypi_dependencies(None);
            let any_in_pypi = specs.keys().any(|name| {
                pypi_deps
                    .names()
                    .any(|pypi_name| pypi_name.as_source() == name.as_source())
            });

            if any_in_pypi {
                dep_type = DependencyType::PypiDependency;
            }
        }
    }

    match dep_type {
        DependencyType::CondaDependency(spec_type) => {
            workspace_ctx
                .remove_conda_deps(
                    args.dependency_config.specs()?,
                    spec_type,
                    (&args).try_into()?,
                )
                .await?;
        }
        DependencyType::PypiDependency => {
            let pypi_deps = args
                .dependency_config
                .pypi_deps(&workspace)?
                .into_iter()
                .map(|(name, req)| (name, (req, None, None)))
                .collect();
            workspace_ctx
                .remove_pypi_deps(pypi_deps, (&args).try_into()?)
                .await?;
        }
    };

    args.dependency_config
        .display_success_with_type("Removed", Default::default(), dep_type);

    Ok(())
}
