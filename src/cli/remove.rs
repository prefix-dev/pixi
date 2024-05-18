use std::str::FromStr;

use clap::Parser;
use miette::miette;
use pep508_rs::Requirement;

use crate::config::ConfigCli;
use crate::environment::get_up_to_date_prefix;
use crate::project::manifest::python::PyPiPackageName;
use crate::DependencyType;
use crate::Project;

use super::add::DependencyConfig;

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

fn convert_pkg_name<T>(deps: &[String]) -> miette::Result<Vec<T>>
where
    T: FromStr,
{
    deps.iter()
        .map(|dep| {
            T::from_str(dep)
                .map_err(|_| miette!("Can't convert dependency name `{dep}` to package name"))
        })
        .collect()
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let (args, config) = (args.dependency_config, args.config);
    let mut project =
        Project::load_or_else_discover(args.manifest_path.as_deref())?.with_cli_config(config);
    let dependency_type = args.dependency_type();

    match dependency_type {
        DependencyType::PypiDependency => {
            let all_pkg_name = convert_pkg_name::<Requirement>(&args.specs)?;
            for dep in all_pkg_name.iter() {
                let name = PyPiPackageName::from_normalized(dep.clone().name);
                project.manifest.remove_pypi_dependency(
                    &name,
                    &args.platform,
                    &args.feature_name(),
                )?;
            }
        }
        DependencyType::CondaDependency(spec_type) => {
            let all_pkg_name = convert_pkg_name::<rattler_conda_types::MatchSpec>(&args.specs)?;
            for dep in all_pkg_name.iter() {
                // Get name or error on missing name
                let name = dep
                    .clone()
                    .name
                    .ok_or_else(|| miette!("Can't remove dependency without a name: {}", dep))?;
                project.manifest.remove_dependency(
                    &name,
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

    args.display_success("Removed");

    Project::warn_on_discovered_from_env(args.manifest_path.as_deref());
    Ok(())
}
