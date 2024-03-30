use std::collections::HashMap;
use std::path::PathBuf;

use clap::Parser;
use console::Color;
use itertools::Itertools;
use rattler_conda_types::Platform;

use crate::lock_file::UpdateLockFileOptions;
use crate::project::manifest::EnvironmentName;
use crate::Project;

/// Show a tree of project dependencies
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false, long_about=format!(
    "\
    Show a tree of project dependencies\n\
    \n\
    Dependency names highlighted in {} are directly specified in the manifest. \
    {} version numbers are Conda packages where as PyPI version numbers are {}.
    ",
    console::style("green").fg(Color::Green).bold(),
    console::style("Yellow").fg(Color::Yellow),
    console::style("blue").fg(Color::Blue)
))]
pub struct Args {
    /// List only packages matching a regular expression
    #[arg()]
    pub regex: Option<String>,

    /// The platform to list packages for. Defaults to the current platform.
    #[arg(long)]
    pub platform: Option<Platform>,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// The environment to list packages for. Defaults to the default environment.
    #[arg(short, long)]
    pub environment: Option<String>,

    #[clap(flatten)]
    pub lock_file_usage: super::LockFileUsageArgs,

    /// Don't install the environment for pypi solving, only update the lock-file if it can solve without installing.
    #[arg(long)]
    pub no_install: bool,

    /// Invert tree and show what depends on given package in the regex argument
    #[arg(short, long, requires = "regex")]
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

    let dep_map = generate_dependency_map(&locked_deps);

    let direct_deps = direct_dependencies(&environment, &platform);

    if args.invert {
        print_inverted_dependency_tree(&invert_dep_map(&dep_map), &direct_deps, &args.regex)?;
    } else {
        print_dependency_tree(&dep_map, &direct_deps, &args.regex)?;
    }
    Ok(())
}

/// Filter and print an inverted dependency tree
fn print_inverted_dependency_tree(
    inverted_dep_map: &HashMap<String, Package>,
    direct_deps: &Vec<String>,
    regex: &Option<String>,
) -> Result<(), miette::Error> {
    let regex = regex
        .as_ref()
        .ok_or("")
        .map_err(|_| miette::miette!("The -i flag requires a package name."))?;
    let regex = regex::Regex::new(regex).map_err(|_| miette::miette!("Invalid regex pattern"))?;

    let mut root_pkg_names = inverted_dep_map.keys().collect_vec();
    root_pkg_names.retain(|p| regex.is_match(p));

    if root_pkg_names.is_empty() {
        Err(miette::miette!(
            "Nothing depends on the given regular expression",
        ))?;
    }

    for pkg_name in root_pkg_names.iter() {
        if let Some(pkg) = inverted_dep_map.get(*pkg_name) {
            print_package(
                "\n".to_string(),
                pkg,
                direct_deps.contains(&pkg.name),
                false,
            );

            print_inverted_leaf(pkg, String::from(""), inverted_dep_map, direct_deps);
        }
    }

    Ok(())
}

/// Recursively print inverted dependency tree leaf nodes
fn print_inverted_leaf(
    pkg: &Package,
    prefix: String,
    inverted_dep_map: &HashMap<String, Package>,
    direct_deps: &Vec<String>,
) {
    let needed_count = pkg.needed_by.len();
    for (index, needed_name) in pkg.needed_by.iter().enumerate() {
        let last = index == needed_count - 1;
        let symbol = if last {
            UTF8_SYMBOLS.ell
        } else {
            UTF8_SYMBOLS.tee
        };

        if let Some(needed_pkg) = inverted_dep_map.get(needed_name) {
            print_package(
                format!("{prefix}{symbol} "),
                needed_pkg,
                direct_deps.contains(&needed_pkg.name),
                false,
            );

            let new_prefix = if index == needed_count - 1 {
                format!("{}{} ", prefix, UTF8_SYMBOLS.empty)
            } else {
                format!("{}{} ", prefix, UTF8_SYMBOLS.down)
            };

            print_inverted_leaf(needed_pkg, new_prefix, inverted_dep_map, direct_deps)
        }
    }
}

/// Filter and print a top down dependency tree
fn print_dependency_tree(
    dep_map: &HashMap<String, Package>,
    direct_deps: &Vec<String>,
    regex: &Option<String>,
) -> Result<(), miette::Error> {
    let mut filtered_deps = direct_deps.clone();

    if let Some(regex) = regex {
        let regex = regex::Regex::new(regex).map_err(|_| miette::miette!("Invalid regex"))?;
        filtered_deps.retain(|p| regex.is_match(p));

        if filtered_deps.is_empty() {
            Err(miette::miette!(
                "No top level dependencies matched the given regular expression"
            ))?;
        }
    }

    let mut visited_pkgs = Vec::new();
    let direct_dep_count = filtered_deps.len();

    for (index, pkg_name) in filtered_deps.iter().enumerate() {
        visited_pkgs.push(pkg_name.to_owned());

        let last = index == direct_dep_count - 1;
        let symbol = if last {
            UTF8_SYMBOLS.ell
        } else {
            UTF8_SYMBOLS.tee
        };
        if let Some(pkg) = dep_map.get(pkg_name) {
            print_package(format!("{symbol} "), pkg, true, false);

            let prefix = if last {
                UTF8_SYMBOLS.empty
            } else {
                UTF8_SYMBOLS.down
            };
            print_dependency_leaf(
                pkg,
                format!("{} ", prefix),
                dep_map,
                &mut visited_pkgs,
                direct_deps,
            )
        }
    }
    Ok(())
}

/// Recursively print top down dependency tree nodes
fn print_dependency_leaf(
    pkg: &Package,
    prefix: String,
    dep_map: &HashMap<String, Package>,
    visited_pkgs: &mut Vec<String>,
    direct_deps: &Vec<String>,
) {
    let dep_count = pkg.dependencies.len();
    for (index, dep_name) in pkg.dependencies.iter().enumerate() {
        let last = index == dep_count - 1;
        let symbol = if last {
            UTF8_SYMBOLS.ell
        } else {
            UTF8_SYMBOLS.tee
        };

        // skip virtual packages
        if dep_name.starts_with("__") {
            continue;
        }

        if let Some(dep) = dep_map.get(dep_name) {
            let visited = visited_pkgs.contains(&dep.name);
            visited_pkgs.push(dep.name.to_owned());

            print_package(
                format!("{prefix}{symbol} "),
                dep,
                direct_deps.contains(&dep.name),
                visited,
            );

            if visited {
                continue;
            }

            let new_prefix = if last {
                format!("{}{} ", prefix, UTF8_SYMBOLS.empty)
            } else {
                format!("{}{} ", prefix, UTF8_SYMBOLS.down)
            };
            print_dependency_leaf(dep, new_prefix, dep_map, visited_pkgs, direct_deps);
        }
    }
}

/// Print package and style by attributes, like if are a direct dependency (name is green and bold),
/// or by the source of the package (yellow version string for Conda, blue for PyPI).
/// Packages that have already been visited and will not be recursed into again are
/// marked with a star (*).
fn print_package(prefix: String, package: &Package, direct: bool, visited: bool) {
    println!(
        "{}{} v{} {}",
        prefix,
        if direct {
            console::style(&package.name).fg(Color::Green).bold()
        } else {
            console::style(&package.name)
        },
        match package.source {
            PackageSource::Conda => console::style(&package.version).fg(Color::Yellow),
            PackageSource::Pypi => console::style(&package.version).fg(Color::Blue),
        },
        if visited { "(*)" } else { "" }
    );
}

/// Extract the direct Conda and PyPI dependencies from the environment
fn direct_dependencies(
    environment: &crate::project::Environment<'_>,
    platform: &Platform,
) -> Vec<String> {
    let mut project_dependency_names = environment
        .dependencies(None, Some(*platform))
        .names()
        .map(|p| p.as_source().to_string())
        .collect_vec();
    project_dependency_names.extend(
        environment
            .pypi_dependencies(Some(*platform))
            .into_iter()
            .map(|(name, _)| name.as_normalized().to_string()),
    );
    project_dependency_names
}

#[derive(Debug, Copy, Clone)]
enum PackageSource {
    Conda,
    Pypi,
}

#[derive(Debug, Clone)]
struct Package {
    name: String,
    version: String,
    dependencies: Vec<String>,
    needed_by: Vec<String>,
    source: PackageSource,
}

/// Builds a hashmap of dependencies, with names, versions, and what they depend on
fn generate_dependency_map(locked_deps: &Vec<rattler_lock::Package>) -> HashMap<String, Package> {
    let mut dep_map = HashMap::new();

    for dep in locked_deps {
        let version = dep.version().into_owned();

        if let Some(dep) = dep.as_conda() {
            let name = dep.package_record().name.as_normalized().to_string();
            let mut dependencies = Vec::new();
            for d in dep.package_record().depends.iter() {
                if let Some((dep_name, _)) = d.split_once(' ') {
                    dependencies.push(dep_name.to_string())
                }
            }

            dep_map.insert(
                name.clone(),
                Package {
                    name: name.clone(),
                    version,
                    dependencies: dependencies.into_iter().unique().collect(),
                    needed_by: Vec::new(),
                    source: PackageSource::Conda,
                },
            );
        } else if let Some(dep) = dep.as_pypi() {
            let name = dep.data().package.name.to_string();

            let mut dependencies = Vec::new();
            for p in dep.data().package.requires_dist.iter() {
                if let Some(markers) = &p.marker {
                    tracing::info!(
                        "Extra and environment markers currently cannot be parsed on {} which is specified by {}, skipping. {:?}",
                        p.name,
                        name,
                        markers
                    );
                } else {
                    dependencies.push(p.name.to_string())
                }
            }
            dep_map.insert(
                name.clone(),
                Package {
                    name: name.clone(),
                    version,
                    dependencies: dependencies.into_iter().unique().collect(),
                    needed_by: Vec::new(),
                    source: PackageSource::Pypi,
                },
            );
        }
    }
    dep_map
}

/// Given a map of dependencies, invert it so that it has what a package is needed by,
/// rather than what it depends on
fn invert_dep_map(dep_map: &HashMap<String, Package>) -> HashMap<String, Package> {
    let mut inverted_deps = dep_map.clone();

    for pkg in dep_map.values() {
        for dep in pkg.dependencies.iter() {
            if let Some(idep) = inverted_deps.get_mut(dep) {
                idep.needed_by.push(pkg.name.clone());
            }
        }
    }

    inverted_deps
}
