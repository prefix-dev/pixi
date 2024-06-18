use clap::Parser;

use crate::config::ConfigCli;
use crate::environment::get_up_to_date_prefix;
use crate::DependencyType;
use crate::Project;

use super::add::DependencyConfig;
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
    pub dependency_config: DependencyConfig,

    #[clap(flatten)]
    pub config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let (args, config) = (args.dependency_config, args.config);
    let mut project =
        Project::load_or_else_discover(args.manifest_path.as_deref())?.with_cli_config(config);
    let dependency_type = args.dependency_type();

    match dependency_type {
        DependencyType::PypiDependency => {
            for name in args.pypi_deps(&project)?.keys() {
                project.manifest.remove_pypi_dependency(
                    name,
                    &args.platform,
                    &args.feature_name(),
                )?;
            }
        }
        DependencyType::CondaDependency(spec_type) => {
            for name in args.specs()?.keys() {
                project.manifest.remove_dependency(
                    name,
                    spec_type,
                    &args.platform,
                    &args.feature_name(),
                )?;
            }
        }
    };

    project.save()?;

    // TODO: update all environments touched by this feature defined.
    // updating prefix after removing from toml
    get_up_to_date_prefix(
        &project.default_environment(),
        args.lock_file_usage(),
        args.no_install,
    )
    .await?;

    args.display_success("Removed", Default::default());

    Project::warn_on_discovered_from_env(args.manifest_path.as_deref());
    Ok(())
}
