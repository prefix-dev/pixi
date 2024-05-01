use std::path::PathBuf;
use std::str::FromStr;

use clap::Parser;
use indexmap::IndexMap;
use miette::miette;
use pep508_rs::Requirement;
use rattler_conda_types::Platform;

use crate::config::ConfigCli;
use crate::environment::{get_up_to_date_prefix, LockFileUsage};
use crate::project::manifest::python::PyPiPackageName;
use crate::project::manifest::FeatureName;
use crate::{consts, project::SpecType, Project};

/// Remove the dependency from the project
#[derive(Debug, Default, Parser)]
pub struct Args {
    /// List of dependencies you wish to remove from the project
    #[arg(required = true)]
    pub deps: Vec<String>,

    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Whether dependency is a host dependency
    #[arg(long, conflicts_with = "build")]
    pub host: bool,

    /// Whether dependency is a build dependency
    #[arg(long, conflicts_with = "host")]
    pub build: bool,

    /// Whether the dependency is a pypi package
    #[arg(long)]
    pub pypi: bool,

    /// Don't install the environment, only remove the package from the lock-file and manifest.
    #[arg(long)]
    pub no_install: bool,

    /// The platform for which the dependency should be removed
    #[arg(long, short)]
    pub platform: Option<Platform>,

    /// The feature for which the dependency should be removed
    #[arg(long, short)]
    pub feature: Option<String>,

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
    let mut project =
        Project::load_or_else_discover(args.manifest_path.as_deref())?.with_cli_config(args.config);
    let deps = args.deps;
    let spec_type = if args.host {
        SpecType::Host
    } else if args.build {
        SpecType::Build
    } else {
        SpecType::Run
    };

    let section_name: String = if args.pypi {
        consts::PYPI_DEPENDENCIES.to_string()
    } else {
        spec_type.name().to_string()
    };
    let table_name = if let Some(p) = &args.platform {
        format!("target.{}.{}", p.as_str(), section_name)
    } else {
        section_name
    };
    let feature_name = args
        .feature
        .map_or(FeatureName::Default, FeatureName::Named);

    fn format_ok_message(pkg_name: &str, pkg_extras: &str, table_name: &str) -> String {
        format!(
            "Removed {} from [{}]",
            console::style(format!("{pkg_name} {pkg_extras}")).bold(),
            console::style(table_name).bold()
        )
    }
    let mut sucessful_output: Vec<String> = Vec::with_capacity(deps.len());
    if args.pypi {
        let all_pkg_name = convert_pkg_name::<Requirement>(&deps)?;
        for dep in all_pkg_name.iter() {
            let name = PyPiPackageName::from_normalized(dep.clone().name);
            let (name, req) =
                project
                    .manifest
                    .remove_pypi_dependency(&name, args.platform, &feature_name)?;
            sucessful_output.push(format_ok_message(
                name.as_source(),
                &req.to_string(),
                &table_name,
            ));
        }
    } else {
        let all_pkg_name = convert_pkg_name::<rattler_conda_types::MatchSpec>(&deps)?;
        for dep in all_pkg_name.iter() {
            // Get name or error on missing name
            let name = dep
                .clone()
                .name
                .ok_or_else(|| miette!("Can't remove dependency without a name: {}", dep))?;
            let (name, req) = project.manifest.remove_dependency(
                &name,
                spec_type,
                args.platform,
                &feature_name,
            )?;
            sucessful_output.push(format_ok_message(
                name.as_source(),
                &req.to_string(),
                &table_name,
            ));
        }
    };

    project.save()?;
    eprintln!("{}", sucessful_output.join("\n"));

    // TODO: update all environments touched by this feature defined.
    // updating prefix after removing from toml
    get_up_to_date_prefix(
        &project.default_environment(),
        LockFileUsage::Update,
        args.no_install,
        IndexMap::default(),
    )
    .await?;

    Project::warn_on_discovered_from_env(args.manifest_path.as_deref());
    Ok(())
}
