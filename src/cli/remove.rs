use std::path::PathBuf;

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
    PixiDeps(miette::Result<(String, NamelessMatchSpec)>),
    PyPiDeps(miette::Result<(rip::types::PackageName, PyPiRequirement)>),
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
                DependencyRemovalResult::PyPiDeps(project.manifest.remove_pypi_dependency(dep))
            } else {
                DependencyRemovalResult::PixiDeps(if let Some(p) = &args.platform {
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

    for result in results.iter() {
        let removed = match result {
            DependencyRemovalResult::PixiDeps(pixi_result) => {
                pixi_result.as_ref().unwrap().0.to_string()
            }
            DependencyRemovalResult::PyPiDeps(pypi_result) => {
                pypi_result.as_ref().unwrap().0.as_str().to_string()
            }
        };
        let spec = match result {
            DependencyRemovalResult::PixiDeps(pixi_result) => {
                pixi_result.as_ref().unwrap().1.to_string()
            }
            DependencyRemovalResult::PyPiDeps(pypi_result) => {
                pypi_result.as_ref().unwrap().1.to_string()
            }
        };

        let table_name = if let Some(p) = &args.platform {
            format!("target.{}.{}", p.as_str(), spec_type.name())
        } else {
            spec_type.name().to_string()
        };

        eprintln!(
            "Removed {} from [{}]",
            console::style(format!("{removed} {spec}")).bold(),
            console::style(table_name).bold(),
        );
    }

    for result in &results {
        match result {
            DependencyRemovalResult::PixiDeps(Err(e))
            | DependencyRemovalResult::PyPiDeps(Err(e)) => {
                eprintln!("{e}");
            }
            _ => {}
        }
    }

    Ok(())
}
