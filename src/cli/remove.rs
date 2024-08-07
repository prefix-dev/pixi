use clap::Parser;

use crate::environment::get_up_to_date_prefix;
use crate::DependencyType;
use crate::Project;

use crate::cli::cli_config::{DependencyConfig, PrefixUpdateConfig, ProjectConfig};

use super::has_specs::HasSpecs;

/// Removes dependencies from the project
///
///  If the project manifest is a `pyproject.toml`, removing a pypi dependency with the `--pypi` flag will remove it from either
/// - the native pyproject `project.dependencies` array or, if a feature is specified, the native `project.optional-dependencies` table
/// - pixi `pypi-dependencies` tables of the default feature or, if a feature is specified, a named feature
///
#[derive(Debug, Default, Parser)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: ProjectConfig,

    #[clap(flatten)]
    pub dependency_config: DependencyConfig,

    #[clap(flatten)]
    pub prefix_update_config: PrefixUpdateConfig,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let (dependency_config, prefix_update_config, project_config) = (
        args.dependency_config,
        args.prefix_update_config,
        args.project_config,
    );

    let mut project = Project::load_or_else_discover(project_config.manifest_path.as_deref())?
        .with_cli_config(prefix_update_config.config.clone());
    let dependency_type = dependency_config.dependency_type();

    match dependency_type {
        DependencyType::PypiDependency => {
            for name in dependency_config.pypi_deps(&project)?.keys() {
                project.manifest.remove_pypi_dependency(
                    name,
                    &dependency_config.platform,
                    &dependency_config.feature_name(),
                )?;
            }
        }
        DependencyType::CondaDependency(spec_type) => {
            for name in dependency_config.specs()?.keys() {
                project.manifest.remove_dependency(
                    name,
                    spec_type,
                    &dependency_config.platform,
                    &dependency_config.feature_name(),
                )?;
            }
        }
    };

    project.save()?;

    // TODO: update all environments touched by this feature defined.
    // updating prefix after removing from toml
    if !prefix_update_config.no_lockfile_update {
        get_up_to_date_prefix(
            &project.default_environment(),
            prefix_update_config.lock_file_usage(),
            prefix_update_config.no_install,
        )
        .await?;
    }

    dependency_config.display_success("Removed", Default::default());

    Project::warn_on_discovered_from_env(project_config.manifest_path.as_deref());
    Ok(())
}
