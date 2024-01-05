use std::path::PathBuf;
use std::str::FromStr;

use clap::Parser;
use miette::miette;
use rattler_conda_types::Platform;

use crate::environment::LockFileUsage;
use crate::{consts, environment::get_up_to_date_prefix, project::SpecType, Project};

/// Remove the dependency from the project
#[derive(Debug, Default, Parser)]
pub struct Args {
    /// List of dependencies you wish to remove from the project
    #[arg(required = true)]
    pub deps: Vec<String>,

    /// The path to 'pixi.toml'
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

    /// The platform for which the dependency should be removed
    #[arg(long, short)]
    pub platform: Option<Platform>,
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
    let mut project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
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

    fn print_ok_dep_removal(pkg_name: &String, pkg_extras: &String, table_name: &String) {
        eprintln!(
            "Removed {} from [{}]",
            console::style(format!("{pkg_name} {pkg_extras}")).bold(),
            console::style(table_name).bold()
        )
    }

    if args.pypi {
        let all_pkg_name = convert_pkg_name::<rip::types::PackageName>(&deps)?;
        for dep in all_pkg_name.iter() {
            let result = if let Some(p) = &args.platform {
                project.manifest.remove_target_pypi_dependency(dep, p)?
            } else {
                project.manifest.remove_pypi_dependency(dep)?
            };
            print_ok_dep_removal(
                &result.0.as_str().to_string(),
                &result.1.to_string(),
                &table_name,
            );
        }
    } else {
        let all_pkg_name = convert_pkg_name::<rattler_conda_types::PackageName>(&deps)?;
        for dep in all_pkg_name.iter() {
            let result = if let Some(p) = &args.platform {
                project
                    .manifest
                    .remove_target_dependency(dep, &spec_type, p)?
            } else {
                project.manifest.remove_dependency(dep, &spec_type)?
            };
            print_ok_dep_removal(&result.0, &result.1.to_string(), &table_name);
        }
    };
    project.save()?;

    // updating prefix after removing from toml
    let _ = get_up_to_date_prefix(&project, LockFileUsage::Update, false, None).await?;
    Ok(())
}
