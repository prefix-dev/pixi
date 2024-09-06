use std::collections::HashMap;

use ahash::{HashSet, HashSetExt};
use clap::Parser;
use console::Color;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use pixi_manifest::FeaturesExt;
use rattler_conda_types::Platform;

use crate::{
    cli::cli_config::{PrefixUpdateConfig, ProjectConfig},
    lock_file::UpdateLockFileOptions,
    Project,
};

/// Show a tree of project dependencies
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false, long_about = format!(
    "\
    Show a tree of project dependencies\n\
    \n\
    Dependency names highlighted in {} are directly specified in the manifest. \
    {} version numbers are conda packages, PyPI version numbers are {}.
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
    #[arg(long, short)]
    pub platform: Option<Platform>,

    #[clap(flatten)]
    pub project_config: ProjectConfig,

    /// The environment to list packages for. Defaults to the default
    /// environment.
    #[arg(short, long)]
    pub environment: Option<String>,

    #[clap(flatten)]
    pub prefix_update_config: PrefixUpdateConfig,

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
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?;
    let environment = project.environment_from_name_or_env_var(args.environment)?;
    let lock_file = project
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: args.prefix_update_config.lock_file_usage(),
            no_install: args.prefix_update_config.no_install,
            ..UpdateLockFileOptions::default()
        })
        .await?;
    let platform = args.platform.unwrap_or_else(|| environment.best_platform());
    let locked_deps = lock_file
        .lock_file
        .environment(environment.name().as_str())
        .and_then(|env| env.packages(platform).map(Vec::from_iter))
        .unwrap_or_default();

    let dep_map = generate_dependency_map(&locked_deps);

    let direct_deps = direct_dependencies(&environment, &platform, &dep_map);

    if !environment.is_default() {
        eprintln!("Environment: {}", environment.name().fancy_display());
    }

    if args.invert {
        print_inverted_dependency_tree(&invert_dep_map(&dep_map), &direct_deps, &args.regex)?;
    } else {
        print_dependency_tree(&dep_map, &direct_deps, &args.regex)?;
    }
    Project::warn_on_discovered_from_env(args.project_config.manifest_path.as_deref());
    Ok(())
}

/// Filter and print an inverted dependency tree
fn print_inverted_dependency_tree(
    inverted_dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
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

    let mut visited_pkgs = HashSet::new();
    for pkg_name in root_pkg_names {
        if let Some(pkg) = inverted_dep_map.get(pkg_name) {
            let visited = !visited_pkgs.insert(pkg_name.clone());
            print_package("\n", pkg, direct_deps.contains(&pkg.name), visited);

            if !visited {
                print_inverted_leaf(
                    pkg,
                    String::from(""),
                    inverted_dep_map,
                    direct_deps,
                    &mut visited_pkgs,
                );
            }
        }
    }

    Ok(())
}

/// Recursively print inverted dependency tree leaf nodes
fn print_inverted_leaf(
    pkg: &Package,
    prefix: String,
    inverted_dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    visited_pkgs: &mut HashSet<String>,
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
            let visited = !visited_pkgs.insert(needed_pkg.name.clone());
            print_package(
                &format!("{prefix}{symbol} "),
                needed_pkg,
                direct_deps.contains(&needed_pkg.name),
                visited,
            );

            if !visited {
                let new_prefix = if index == needed_count - 1 {
                    format!("{}{} ", prefix, UTF8_SYMBOLS.empty)
                } else {
                    format!("{}{} ", prefix, UTF8_SYMBOLS.down)
                };

                print_inverted_leaf(
                    needed_pkg,
                    new_prefix,
                    inverted_dep_map,
                    direct_deps,
                    visited_pkgs,
                )
            }
        }
    }
}

/// Print a transitive dependency tree
fn print_transitive_dependency_tree(
    dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    filtered_keys: Vec<String>,
) -> Result<(), miette::Error> {
    let mut visited_pkgs = Vec::new();

    for pkg_name in filtered_keys.iter() {
        visited_pkgs.push(pkg_name.clone());

        if let Some(pkg) = dep_map.get(pkg_name) {
            print_package("\n", pkg, direct_deps.contains(&pkg.name), false);

            print_dependency_leaf(pkg, "".to_string(), dep_map, &mut visited_pkgs, direct_deps)
        }
    }
    Ok(())
}

/// Filter and print a top down dependency tree
fn print_dependency_tree(
    dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    regex: &Option<String>,
) -> Result<(), miette::Error> {
    let mut filtered_deps = direct_deps.clone();

    if let Some(regex) = regex {
        let regex = regex::Regex::new(regex).map_err(|_| miette::miette!("Invalid regex"))?;
        filtered_deps.retain(|p| regex.is_match(p));

        if filtered_deps.is_empty() {
            let mut filtered_keys = dep_map.keys().map(|p| p.to_owned()).collect_vec();
            filtered_keys.retain(|p| regex.is_match(p));

            if filtered_keys.is_empty() {
                Err(miette::miette!(
                    "No dependencies matched the given regular expression"
                ))?;
            }

            tracing::info!("No top level dependencies matched the regular expression, showing matching transitive dependencies");

            return print_transitive_dependency_tree(dep_map, direct_deps, filtered_keys);
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
            print_package(
                &format!("{symbol} "),
                pkg,
                direct_deps.contains(&pkg.name),
                false,
            );

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
    direct_deps: &HashSet<String>,
) {
    let dep_count = pkg.dependencies.len();
    for (index, dep_name) in pkg.dependencies.iter().enumerate() {
        let last = index == dep_count - 1;
        let symbol = if last {
            UTF8_SYMBOLS.ell
        } else {
            UTF8_SYMBOLS.tee
        };

        if let Some(dep) = dep_map.get(dep_name) {
            let visited = visited_pkgs.contains(&dep.name);
            visited_pkgs.push(dep.name.to_owned());

            print_package(
                &format!("{prefix}{symbol} "),
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
        } else {
            let visited = visited_pkgs.contains(dep_name);
            visited_pkgs.push(dep_name.to_owned());

            print_package(
                &format!("{prefix}{symbol} "),
                &Package {
                    name: dep_name.to_owned(),
                    version: String::from(""),
                    dependencies: Vec::new(),
                    needed_by: Vec::new(),
                    source: PackageSource::Conda,
                },
                false,
                visited,
            )
        }
    }
}

/// Print package and style by attributes, like if are a direct dependency (name
/// is green and bold), or by the source of the package (yellow version string
/// for Conda, blue for PyPI). Packages that have already been visited and will
/// not be recursed into again are marked with a star (*).
fn print_package(prefix: &str, package: &Package, direct: bool, visited: bool) {
    println!(
        "{}{} {} {}",
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
    dep_map: &HashMap<String, Package>,
) -> HashSet<String> {
    let mut project_dependency_names = environment
        .dependencies(None, Some(*platform))
        .names()
        .filter(|p| {
            if let Some(value) = dep_map.get(p.as_source()) {
                value.source == PackageSource::Conda
            } else {
                false
            }
        })
        .map(|p| p.as_source().to_string())
        .collect::<HashSet<_>>();

    project_dependency_names.extend(
        environment
            .pypi_dependencies(Some(*platform))
            .into_iter()
            .filter(|(name, _)| {
                if let Some(value) = dep_map.get(&*name.as_normalized().as_dist_info_name()) {
                    value.source == PackageSource::Pypi
                } else {
                    false
                }
            })
            .map(|(name, _)| name.as_normalized().as_dist_info_name().into_owned()),
    );
    project_dependency_names
}

#[derive(Debug, Copy, Clone, PartialEq)]
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

/// Builds a hashmap of dependencies, with names, versions, and what they depend
/// on
fn generate_dependency_map(locked_deps: &Vec<rattler_lock::Package>) -> HashMap<String, Package> {
    let mut package_dependencies_map = HashMap::new();

    for package in locked_deps {
        let version = package.version().into_owned();

        if let Some(conda_package) = package.as_conda() {
            let name = conda_package
                .package_record()
                .name
                .as_normalized()
                .to_string();
            // Parse the dependencies of the package
            let dependencies: Vec<String> = conda_package
                .package_record()
                .depends
                .iter()
                .map(|d| {
                    d.split_once(' ')
                        .map_or_else(|| d.to_string(), |(dep_name, _)| dep_name.to_string())
                })
                .collect();

            package_dependencies_map.insert(
                name.clone(),
                Package {
                    name: name.clone(),
                    version,
                    dependencies: dependencies.into_iter().unique().collect(),
                    needed_by: Vec::new(),
                    source: PackageSource::Conda,
                },
            );
        } else if let Some(pypi_package) = package.as_pypi() {
            let name = pypi_package
                .data()
                .package
                .name
                .as_dist_info_name()
                .into_owned();

            let mut dependencies = Vec::new();
            for p in pypi_package.data().package.requires_dist.iter() {
                // If this is not true, it means that the marker does not hold for every environment
                if !&p.marker.is_true() {
                    tracing::info!(
                        "Extra and environment markers currently cannot be parsed on {} which is specified by {}, skipping. {:?}",
                        p.name,
                        name,
                        p.marker
                    );
                } else {
                    dependencies.push(p.name.as_dist_info_name().into_owned())
                }
            }
            package_dependencies_map.insert(
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
    package_dependencies_map
}

/// Given a map of dependencies, invert it so that it has what a package is
/// needed by, rather than what it depends on
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
