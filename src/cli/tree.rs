use std::collections::HashMap;
use std::path::PathBuf;

use clap::Parser;
use itertools::Itertools;
use rattler_conda_types::Platform;

use crate::lock_file::UpdateLockFileOptions;
use crate::project::manifest::EnvironmentName;
use crate::Project;

// Show a tree of project dependencies
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    /// List only packages matching a regular expression
    #[arg()]
    pub regex: Option<String>,

    /// The platform to list packages for. Defaults to the current platform.
    #[arg(long)]
    pub platform: Option<Platform>,

    /// The path to 'pixi.toml'
    #[arg(long, env = "PIXI_PROJECT_MANIFEST")]
    pub manifest_path: Option<PathBuf>,

    /// The environment to list packages for. Defaults to the default environment.
    #[arg(short, long)]
    pub environment: Option<String>,

    #[clap(flatten)]
    pub lock_file_usage: super::LockFileUsageArgs,

    /// Don't install the environment for pypi solving, only update the lock-file if it can solve without installing.
    #[arg(long)]
    pub no_install: bool,

    /// Invert tree and show what depends on given package
    #[arg(short, long)]
    pub invert: bool,
}

struct Symbols {
    down: &'static str,
    tee: &'static str,
    ell: &'static str,
    // right: &'static str,
    empty: &'static str,
}

static UTF8_SYMBOLS: Symbols = Symbols {
    down: "│  ",
    tee: "├──",
    ell: "└──",
    // right: "───",
    empty: "   ",
};

pub async fn execute(args: Args) -> miette::Result<()> {
    if args.invert {
        print_inverted_tree(args).await?;
    } else {
        print_tree(args).await?;
    }
    Ok(())
}

#[derive(Debug)]
struct InvertedPackage {
    needed_by: Vec<String>,
}

// Prints an inverted tree which requires a regex
async fn print_inverted_tree(args: Args) -> Result<(), miette::Error> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    let environment_name = args
        .environment
        .map_or_else(|| EnvironmentName::Default, EnvironmentName::Named);
    let environment = project
        .environment(&environment_name)
        .ok_or_else(|| miette::miette!("unknown environment '{environment_name}'"))?;
    let lock_file = project
        .up_to_date_lock_file(UpdateLockFileOptions {
            lock_file_usage: args.lock_file_usage.into(),
            no_install: args.no_install,
            ..UpdateLockFileOptions::default()
        })
        .await?;
    let platform = args.platform.unwrap_or_else(Platform::current);
    let locked_deps = lock_file
        .lock_file
        .environment(environment.name().as_str())
        .and_then(|env| env.packages(platform).map(Vec::from_iter))
        .unwrap_or_default();
    let conda_records = locked_deps.iter().filter_map(|d| d.as_conda());

    let mut needed_map = HashMap::new();

    for rec in conda_records {
        let package_record = rec.package_record();

        for dep in package_record.depends.iter() {
            if let Some((dep_name, _)) = dep.split_once(' ') {
                let package = needed_map
                    .entry(dep_name)
                    .or_insert(InvertedPackage { needed_by: vec![] });
                package
                    .needed_by
                    .push(package_record.name.as_source().to_string());
            }
        }
    }

    let mut root_package_names: Vec<&&str> = needed_map.keys().collect();

    let regex = args
        .regex
        .ok_or("The `-i` flag requires a package name.")
        .map_err(|_| miette::miette!("The `-i` flag requires a package name."))?;
    let regex = regex::Regex::new(&regex).map_err(|_| miette::miette!("Invalid regex"))?;
    root_package_names.retain(|p| regex.is_match(p));

    if root_package_names.is_empty() {
        println!("Nothing depends on the given regular expression");
        return Ok(());
    }

    for pkg_name in root_package_names {
        println!("\n{}", pkg_name);

        let package = needed_map.get(pkg_name).unwrap();

        let needed_count = package.needed_by.len();
        for (index, needed_by) in package.needed_by.iter().enumerate() {
            let symbol = if index == needed_count - 1 {
                UTF8_SYMBOLS.ell
            } else {
                UTF8_SYMBOLS.tee
            };
            println!("{} {}", symbol, needed_by);

            let prefix = if index == needed_count - 1 {
                UTF8_SYMBOLS.empty
            } else {
                UTF8_SYMBOLS.down
            };

            print_needed_by(needed_by, format!("{} ", prefix), &needed_map);
        }
    }

    Ok(())
}

// Recursively print what a package is needed by as part of an inverted tree
fn print_needed_by(
    package_name: &str,
    prefix: String,
    needed_map: &HashMap<&str, InvertedPackage>,
) {
    if let Some(package) = needed_map.get(&package_name) {
        let needed_count = package.needed_by.len();
        for (index, needed_by) in package.needed_by.iter().enumerate() {
            let symbol = if index == needed_count - 1 {
                UTF8_SYMBOLS.ell
            } else {
                UTF8_SYMBOLS.tee
            };
            println!("{}{} {}", prefix, symbol, needed_by);

            let new_prefix = if index == needed_count - 1 {
                format!("{}{} ", prefix, UTF8_SYMBOLS.empty)
            } else {
                format!("{}{} ", prefix, UTF8_SYMBOLS.down)
            };

            print_needed_by(needed_by, new_prefix, needed_map);
        }
    }
}

#[derive(Debug)]
struct Dependency {
    name: String,
}

#[derive(Debug)]
struct TreePackage {
    dependencies: Vec<Dependency>,
    version: String,
}

// Print a top down dependency tree
async fn print_tree(args: Args) -> Result<(), miette::Error> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    let environment_name = args
        .environment
        .map_or_else(|| EnvironmentName::Default, EnvironmentName::Named);
    let environment = project
        .environment(&environment_name)
        .ok_or_else(|| miette::miette!("unknown environment '{environment_name}'"))?;
    let lock_file = project
        .up_to_date_lock_file(UpdateLockFileOptions {
            lock_file_usage: args.lock_file_usage.into(),
            no_install: args.no_install,
            ..UpdateLockFileOptions::default()
        })
        .await?;
    let platform = args.platform.unwrap_or_else(Platform::current);
    let locked_deps = lock_file
        .lock_file
        .environment(environment.name().as_str())
        .and_then(|env| env.packages(platform).map(Vec::from_iter))
        .unwrap_or_default();
    let conda_records = locked_deps.iter().filter_map(|d| d.as_conda());
    let mut dependency_map = HashMap::new();

    for rec in conda_records {
        let package_record = rec.package_record();

        let mut dependencies = Vec::new();

        for dep in package_record.depends.iter() {
            if let Some((dep_name, _)) = dep.split_once(' ') {
                dependencies.push(Dependency {
                    name: dep_name.to_string(),
                });
            }
        }

        dependency_map.insert(
            package_record.name.as_source(),
            TreePackage {
                dependencies,
                version: package_record.version.as_str().to_string(),
            },
        );
    }

    let mut project_dependency_names = environment
        .dependencies(None, Some(platform))
        .names()
        .map(|p| p.as_source().to_string())
        .collect_vec();

    if let Some(regex) = args.regex {
        let regex = regex::Regex::new(&regex).map_err(|_| miette::miette!("Invalid regex"))?;
        project_dependency_names.retain(|p| regex.is_match(p));

        if project_dependency_names.is_empty() {
            Err(miette::miette!(
                "No top level dependencies matched the given regular expression"
            ))?;
        }
    }

    let mut visited_dependencies = Vec::new();
    let project_dependency_count = project_dependency_names.len();
    for (index, pkg_name) in project_dependency_names.iter().enumerate() {
        visited_dependencies.push(pkg_name.to_owned());
        let symbol = if index == project_dependency_count - 1 {
            UTF8_SYMBOLS.ell
        } else {
            UTF8_SYMBOLS.tee
        };
        let dep = dependency_map.get(&pkg_name.as_str()).unwrap();

        println!("{} {} v{}", symbol, pkg_name, dep.version);

        let prefix = if index == project_dependency_count - 1 {
            UTF8_SYMBOLS.empty
        } else {
            UTF8_SYMBOLS.down
        };
        print_dependencies(
            dep,
            format!("{} ", prefix),
            &dependency_map,
            &mut visited_dependencies,
        );
    }
    Ok(())
}

// Recursively print the dependencies in a regular tree
fn print_dependencies(
    package: &TreePackage,
    prefix: String,
    dependency_map: &HashMap<&str, TreePackage>,
    visited_dependencies: &mut Vec<String>,
) {
    let dep_count = package.dependencies.len();
    for (index, pkg_name) in package
        .dependencies
        .iter()
        .map(|d| d.name.clone())
        .enumerate()
    {
        let symbol = if index == dep_count - 1 {
            UTF8_SYMBOLS.ell
        } else {
            UTF8_SYMBOLS.tee
        };

        // Skip virtual packages
        if pkg_name.starts_with("__") {
            continue;
        }

        let dep = dependency_map.get(&pkg_name.as_str()).unwrap();
        let visited = visited_dependencies.contains(&pkg_name);
        visited_dependencies.push(pkg_name.as_str().to_owned());

        println!(
            "{}{} {} v{} {}",
            prefix,
            symbol,
            pkg_name,
            dep.version,
            if visited { "(*)" } else { "" }
        );

        let new_prefix = if index == dep_count - 1 {
            format!("{}{} ", prefix, UTF8_SYMBOLS.empty)
        } else {
            format!("{}{} ", prefix, UTF8_SYMBOLS.down)
        };

        if visited {
            continue;
        }
        print_dependencies(dep, new_prefix, dependency_map, visited_dependencies);
    }
}
