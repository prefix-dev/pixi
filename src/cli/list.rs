use std::path::PathBuf;

use clap::Parser;
use rattler_conda_types::Platform;
use rattler_lock::LockedDependencyKind;
use serde::Serialize;

use crate::lock_file::load_lock_file;
use crate::project::SpecType;
use crate::Project;

/// List project's packages. Highlighted packages are explicit dependencies.
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    /// The platform to list packages for. Defaults to the current platform.
    #[arg(long)]
    pub platform: Option<Platform>,

    /// Whether to output in json format
    #[arg(long)]
    pub json: bool,

    /// Whether to output in pretty json format
    #[arg(long)]
    pub json_pretty: bool,

    /// Whether to sort the package list by name
    #[arg(long)]
    pub no_sort: bool,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

#[derive(Serialize)]
struct PackageToOutput {
    name: String,
    version: String,
    build: Option<String>,
    kind: String,
    // channel: String,
    is_explicit: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Load the project
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())
        .ok()
        .ok_or_else(|| miette::miette!("No project found"))?;

    // Load the platform
    let platform = args.platform.unwrap_or_else(Platform::current);

    // Load the environment
    // NOTE(hadim): make it an argument once environments are implemented.
    // let environment = project.default_environment();

    // Load the lockfile
    let lockfile = load_lock_file(&project)
        .await
        .map_err(|_| miette::miette!("Cannot load lockfile. Did you run `pixi install` first?"))?;

    let locked_deps = lockfile.packages_for_platform(platform).collect::<Vec<_>>();

    // Get the explicit project dependencies
    let project_dependency_names: Vec<String> = {
        let dependencies = project
            .default_environment()
            .dependencies(Some(SpecType::Run), Some(platform));
        dependencies
            .names()
            .map(|p| p.as_source().to_string())
            .collect()
    };

    // Convert the the list of package record to specific output format
    let mut packages_to_output = locked_deps
        .iter()
        .map(|p| {
            match p.kind {
                // Conda package
                LockedDependencyKind::Conda(_) => {
                    let name = p.name.clone();
                    let version = p.version.clone();
                    let kind = "conda".to_string();
                    let build = p.as_conda().unwrap().build.clone();
                    let is_explicit = project_dependency_names.contains(&name);

                    PackageToOutput {
                        name,
                        version,
                        build,
                        kind,
                        is_explicit,
                    }
                }
                // Pypi package
                LockedDependencyKind::Pypi(_) => {
                    let name = p.name.clone();
                    let version = p.version.clone();
                    let kind = "pypi".to_string();
                    let build = p.as_pypi().unwrap().build.clone();
                    let is_explicit = project_dependency_names.contains(&name);

                    PackageToOutput {
                        name,
                        version,
                        build,
                        kind,
                        is_explicit,
                    }
                }
            }
        })
        .collect::<Vec<_>>();

    // Sort packages by name if needed
    if !args.no_sort {
        // Sort packages by name
        packages_to_output.sort_by(|a, b| a.name.cmp(&b.name));
    }

    // Print as table string or JSON
    if args.json {
        // print packages as json
        json_packages(packages_to_output, args.json_pretty);
    } else {
        // print packages as table
        print_packages(packages_to_output);
    }

    Ok(())
}

fn print_packages(packages: Vec<PackageToOutput>) {
    println!(
        "{:40} {:19} {:19} {:19}",
        console::style("Package").bold(),
        console::style("Version").bold(),
        console::style("Build").bold(),
        console::style("Kind").bold(),
    );

    for package in packages {
        println!(
            "{:40} {:19} {:19} {:19}",
            if package.is_explicit {
                console::style(package.name).green().bright().bold()
            } else {
                console::style(package.name)
            },
            console::style(package.version),
            console::style(package.build.unwrap_or_else(|| "".to_string())),
            console::style(package.kind),
        );
    }
}

fn json_packages(packages: Vec<PackageToOutput>, json_pretty: bool) {
    let json_string = if json_pretty {
        serde_json::to_string_pretty(&packages)
    } else {
        serde_json::to_string(&packages)
    }
    .map_err(|_| miette::miette!("Cannot serialize packages to JSON"))
    .unwrap();

    println!("{}", json_string);
}
