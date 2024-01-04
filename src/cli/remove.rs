use std::path::PathBuf;
use std::str::FromStr;

use clap::Parser;
use rattler_conda_types::{NamelessMatchSpec, PackageName, Platform};

use crate::environment::LockFileUsage;
use crate::project::python::PyPiRequirement;
use crate::{environment::get_up_to_date_prefix, project::SpecType, Project};

/// Remove the dependency from the project
#[derive(Debug, Default, Parser)]
pub struct Args {
    /// List of dependencies you wish to remove from the project
    #[arg(required = true)]
    pub deps: Vec<PackageName>,

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

enum DependencyRemovalResult {
    Conda(miette::Result<(String, NamelessMatchSpec)>),
    PyPi(miette::Result<(rip::types::PackageName, PyPiRequirement)>),
    Error(miette::Report),
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

    let results = deps
        .iter()
        .map(|dep| {
            if args.pypi {
                match rip::types::PackageName::from_str(dep.as_source()) {
                    Ok(pkg_name) => {
                        if let Some(p) = &args.platform {
                            DependencyRemovalResult::PyPi(
                                project.manifest.remove_target_pypi_dependency(&pkg_name, p),
                            )
                        } else {
                            DependencyRemovalResult::PyPi(
                                project.manifest.remove_pypi_dependency(&pkg_name),
                            )
                        }
                    }
                    Err(e) => DependencyRemovalResult::Error(e.into()),
                }
            } else {
                DependencyRemovalResult::Conda(if let Some(p) = &args.platform {
                    project
                        .manifest
                        .remove_target_dependency(dep, &spec_type, p)
                } else {
                    project.manifest.remove_dependency(dep, &spec_type)
                })
            }
        })
        .collect::<Vec<DependencyRemovalResult>>();

    project.save()?;

    // updating prefix after removing from toml
    let _ = get_up_to_date_prefix(&project, LockFileUsage::Update, false, None).await?;
    let table_name = if let Some(p) = &args.platform {
        format!("target.{}.{}", p.as_str(), spec_type.name())
    } else {
        spec_type.name().to_string()
    };
    fn print_ok_dep_removal(pkg_name: &String, pkg_extras: &String, table_name: &String) {
        eprintln!(
            "Removed {} from [{}]",
            console::style(format!("{pkg_name} {pkg_extras}")).bold(),
            console::style(table_name).bold()
        )
    }
    for result in results.iter() {
        match result {
            DependencyRemovalResult::Conda(Ok(pixi_result)) => print_ok_dep_removal(
                &pixi_result.0.to_string(),
                &pixi_result.1.to_string(),
                &table_name,
            ),
            DependencyRemovalResult::PyPi(Ok(pypi_result)) => print_ok_dep_removal(
                &pypi_result.0.as_str().to_string(),
                &pypi_result.1.to_string(),
                &table_name,
            ),
            DependencyRemovalResult::Conda(Err(e))
            | DependencyRemovalResult::PyPi(Err(e))
            | DependencyRemovalResult::Error(e) => {
                eprintln!("{e}")
            }
        }
    }
    Ok(())
}
