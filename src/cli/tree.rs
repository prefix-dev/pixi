use std::{
    collections::HashMap,
    io::{StdoutLock, Write},
};

use ahash::{HashSet, HashSetExt};
use clap::Parser;
use console::Color;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::{IntoDiagnostic, WrapErr};
use pixi_manifest::FeaturesExt;
use rattler_conda_types::Platform;
use rattler_lock::LockedPackageRef;
use regex::Regex;

use crate::{
    cli::cli_config::{PrefixUpdateConfig, WorkspaceConfig},
    lock_file::UpdateLockFileOptions,
    workspace::Environment,
    WorkspaceLocator,
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
    pub workspace_config: WorkspaceConfig,

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
    empty: &'static str,
}

static UTF8_SYMBOLS: Symbols = Symbols {
    down: "│  ",
    tee: "├──",
    ell: "└──",
    empty: "   ",
};

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let environment = workspace
        .environment_from_name_or_env_var(args.environment)
        .wrap_err("Environment not found")?;

    let lock_file = workspace
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: args.prefix_update_config.lock_file_usage(),
            no_install: args.prefix_update_config.no_install,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        })
        .await
        .wrap_err("Failed to update lock file")?;

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

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    if args.invert {
        print_inverted_dependency_tree(
            &mut handle,
            &invert_dep_map(&dep_map),
            &direct_deps,
            &args.regex,
        )
        .wrap_err("Couldn't print the inverted dependency tree")?;
    } else {
        print_dependency_tree(&mut handle, &dep_map, &direct_deps, &args.regex)
            .wrap_err("Couldn't print the dependency tree")?;
    }
    Ok(())
}

/// Filter and print an inverted dependency tree
fn print_inverted_dependency_tree(
    handle: &mut StdoutLock,
    inverted_dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    regex: &Option<String>,
) -> miette::Result<()> {
    let regex = regex
        .as_ref()
        .ok_or_else(|| miette::miette!("The -i flag requires a package name."))?;

    let regex = Regex::new(regex)
        .into_diagnostic()
        .wrap_err("Invalid regular expression")?;

    let root_pkg_names: Vec<_> = inverted_dep_map
        .keys()
        .filter(|p| regex.is_match(p))
        .collect();

    if root_pkg_names.is_empty() {
        return Err(miette::miette!(
            "Nothing depends on the given regular expression"
        ));
    }

    let mut visited_pkgs = HashSet::new();
    for pkg_name in root_pkg_names {
        if let Some(pkg) = inverted_dep_map.get(pkg_name) {
            let visited = !visited_pkgs.insert(pkg_name.clone());
            print_package(handle, "\n", pkg, direct_deps.contains(&pkg.name), visited)?;

            if !visited {
                print_inverted_leaf(
                    handle,
                    pkg,
                    String::from(""),
                    inverted_dep_map,
                    direct_deps,
                    &mut visited_pkgs,
                )?;
            }
        }
    }

    Ok(())
}

/// Recursively print inverted dependency tree leaf nodes
fn print_inverted_leaf(
    handle: &mut StdoutLock,
    pkg: &Package,
    prefix: String,
    inverted_dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    visited_pkgs: &mut HashSet<String>,
) -> miette::Result<()> {
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
                handle,
                &format!("{prefix}{symbol} "),
                needed_pkg,
                direct_deps.contains(&needed_pkg.name),
                visited,
            )?;

            if !visited {
                let new_prefix = if last {
                    format!("{}{} ", prefix, UTF8_SYMBOLS.empty)
                } else {
                    format!("{}{} ", prefix, UTF8_SYMBOLS.down)
                };

                print_inverted_leaf(
                    handle,
                    needed_pkg,
                    new_prefix,
                    inverted_dep_map,
                    direct_deps,
                    visited_pkgs,
                )?;
            }
        }
    }
    Ok(())
}

/// Print a transitive dependency tree
fn print_transitive_dependency_tree(
    handle: &mut StdoutLock,
    dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    filtered_keys: Vec<String>,
) -> miette::Result<()> {
    let mut visited_pkgs = HashSet::new();

    for pkg_name in filtered_keys.iter() {
        if !visited_pkgs.insert(pkg_name.clone()) {
            continue;
        }

        if let Some(pkg) = dep_map.get(pkg_name) {
            print_package(handle, "\n", pkg, direct_deps.contains(&pkg.name), false)?;

            print_dependency_leaf(
                handle,
                pkg,
                "".to_string(),
                dep_map,
                &mut visited_pkgs,
                direct_deps,
            )?;
        }
    }
    Ok(())
}

/// Filter and print a top-down dependency tree
fn print_dependency_tree(
    handle: &mut StdoutLock,
    dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    regex: &Option<String>,
) -> miette::Result<()> {
    let mut filtered_deps = direct_deps.clone();

    if let Some(regex) = regex {
        let regex = Regex::new(regex)
            .into_diagnostic()
            .wrap_err("Invalid regular expression")?;

        filtered_deps.retain(|p| regex.is_match(p));

        if filtered_deps.is_empty() {
            let mut filtered_keys = dep_map.keys().cloned().collect_vec();
            filtered_keys.retain(|p| regex.is_match(p));

            if filtered_keys.is_empty() {
                return Err(miette::miette!(
                    "No dependencies matched the given regular expression"
                ));
            }

            tracing::info!("No top-level dependencies matched the regular expression, showing matching transitive dependencies");

            return print_transitive_dependency_tree(handle, dep_map, direct_deps, filtered_keys);
        }
    }

    let mut visited_pkgs = HashSet::new();
    let direct_dep_count = filtered_deps.len();

    for (index, pkg_name) in filtered_deps.iter().enumerate() {
        if !visited_pkgs.insert(pkg_name.clone()) {
            continue;
        }

        let last = index == direct_dep_count - 1;
        let symbol = if last {
            UTF8_SYMBOLS.ell
        } else {
            UTF8_SYMBOLS.tee
        };
        if let Some(pkg) = dep_map.get(pkg_name) {
            print_package(
                handle,
                &format!("{symbol} "),
                pkg,
                direct_deps.contains(&pkg.name),
                false,
            )?;

            let prefix = if last {
                UTF8_SYMBOLS.empty
            } else {
                UTF8_SYMBOLS.down
            };
            print_dependency_leaf(
                handle,
                pkg,
                format!("{} ", prefix),
                dep_map,
                &mut visited_pkgs,
                direct_deps,
            )?;
        }
    }
    Ok(())
}

/// Recursively print top-down dependency tree nodes
fn print_dependency_leaf(
    handle: &mut StdoutLock,
    pkg: &Package,
    prefix: String,
    dep_map: &HashMap<String, Package>,
    visited_pkgs: &mut HashSet<String>,
    direct_deps: &HashSet<String>,
) -> miette::Result<()> {
    let dep_count = pkg.dependencies.len();
    for (index, dep_name) in pkg.dependencies.iter().enumerate() {
        let last = index == dep_count - 1;
        let symbol = if last {
            UTF8_SYMBOLS.ell
        } else {
            UTF8_SYMBOLS.tee
        };

        if let Some(dep) = dep_map.get(dep_name) {
            let visited = !visited_pkgs.insert(dep.name.clone());

            print_package(
                handle,
                &format!("{prefix}{symbol} "),
                dep,
                direct_deps.contains(&dep.name),
                visited,
            )?;

            if visited {
                continue;
            }

            let new_prefix = if last {
                format!("{}{} ", prefix, UTF8_SYMBOLS.empty)
            } else {
                format!("{}{} ", prefix, UTF8_SYMBOLS.down)
            };
            print_dependency_leaf(handle, dep, new_prefix, dep_map, visited_pkgs, direct_deps)?;
        } else {
            let visited = !visited_pkgs.insert(dep_name.clone());

            print_package(
                handle,
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
            )?;
        }
    }
    Ok(())
}

/// Print package and style by attributes
fn print_package(
    handle: &mut StdoutLock,
    prefix: &str,
    package: &Package,
    direct: bool,
    visited: bool,
) -> miette::Result<()> {
    writeln!(
        handle,
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
    )
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            // Exit gracefully
            std::process::exit(0);
        } else {
            e
        }
    })
    .into_diagnostic()
    .wrap_err("Failed to write package information")
}

/// Extract the direct Conda and PyPI dependencies from the environment
fn direct_dependencies(
    environment: &Environment<'_>,
    platform: &Platform,
    dep_map: &HashMap<String, Package>,
) -> HashSet<String> {
    let mut project_dependency_names = environment
        .combined_dependencies(Some(*platform))
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

/// Simplified package information extracted from the lock file
struct PackageInfo {
    name: String,
    dependencies: Vec<String>,
    source: PackageSource,
}

/// Helper function to extract package information
fn extract_package_info(package: rattler_lock::LockedPackageRef<'_>) -> Option<PackageInfo> {
    if let Some(conda_package) = package.as_conda() {
        // Extract name
        let name = conda_package.record().name.as_normalized().to_string();

        // Extract dependencies
        let dependencies: Vec<String> = conda_package
            .record()
            .depends
            .iter()
            .map(|d| {
                d.split_once(' ')
                    .map_or_else(|| d.to_string(), |(dep_name, _)| dep_name.to_string())
            })
            .collect();

        Some(PackageInfo {
            name,
            dependencies,
            source: PackageSource::Conda,
        })
    } else if let Some((pypi_package_data, _pypi_env_data)) = package.as_pypi() {
        // Extract name
        let name = pypi_package_data.name.as_dist_info_name().into_owned();

        // Extract dependencies
        let dependencies = pypi_package_data
            .requires_dist
            .iter()
            .filter_map(|p| {
                if p.marker.is_true() {
                    Some(p.name.as_dist_info_name().into_owned())
                } else {
                    tracing::info!(
                        "Skipping {} specified by {} due to marker {:?}",
                        p.name,
                        name,
                        p.marker
                    );
                    None
                }
            })
            .collect();

        Some(PackageInfo {
            name,
            dependencies,
            source: PackageSource::Pypi,
        })
    } else {
        None
    }
}

/// Generate a map of dependencies from a list of locked packages
fn generate_dependency_map(
    locked_deps: &[rattler_lock::LockedPackageRef<'_>],
) -> HashMap<String, Package> {
    let mut package_dependencies_map = HashMap::new();

    for &package in locked_deps {
        if let Some(package_info) = extract_package_info(package) {
            package_dependencies_map.insert(
                package_info.name.clone(),
                Package {
                    name: package_info.name,
                    version: match package {
                        LockedPackageRef::Conda(conda_data) => {
                            conda_data.record().version.to_string()
                        }
                        LockedPackageRef::Pypi(pypi_data, _) => pypi_data.version.to_string(),
                    },
                    dependencies: package_info.dependencies.into_iter().unique().collect(),
                    needed_by: Vec::new(),
                    source: package_info.source,
                },
            );
        }
    }
    package_dependencies_map
}

/// Given a map of dependencies, invert it
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
