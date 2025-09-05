//! This file contains the logic to pretty-print dependency lists in a tree-like structure.

use ahash::{HashSet, HashSetExt};
use console::Color;
use miette::{Context, IntoDiagnostic};
use regex::Regex;
use std::collections::HashMap;
use std::io::{StdoutLock, Write};

/// Defines the source of a package. Global packages can only have Conda dependencies.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum PackageSource {
    Conda,
    Pypi,
}

/// Represents a view of a Package with only the fields required for the tree visualization.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub dependencies: Vec<String>,
    pub needed_by: Vec<String>,
    pub source: PackageSource,
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

/// Prints a hierarchical tree of dependencies to the provided output handle.
/// Direct dependencies are shown at the top level, with their transitive dependencies indented below.
///
/// If a regex pattern is provided, only dependencies matching the pattern will be displayed.
/// When no direct dependencies match the pattern but transitive ones do, it shows matching transitive dependencies instead.
///
/// # Arguments
///
/// * `handle` - A mutable lock on stdout for writing the output
/// * `dep_map` - A map of package names to their dependency information
/// * `direct_deps` - A set of package names that should be highlighted.
/// * `regex` - Optional regex pattern to filter which dependencies to display
///
/// # Returns
///
/// Returns `Ok(())` if the tree was printed successfully, or an error if the regex was invalid or no matches were found
pub fn print_dependency_tree(
    handle: &mut StdoutLock,
    dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    regex: &Option<String>,
) -> miette::Result<()> {
    let mut filtered_deps = direct_deps.clone();
    let mut transitive = false;

    if let Some(regex) = regex {
        let regex = Regex::new(regex)
            .into_diagnostic()
            .wrap_err("Invalid regular expression")?;

        filtered_deps.retain(|p| regex.is_match(p));

        if filtered_deps.is_empty() {
            filtered_deps = dep_map.keys().cloned().collect();
            filtered_deps.retain(|p| regex.is_match(p));

            if filtered_deps.is_empty() {
                return Err(miette::miette!(
                    "No dependencies matched the given regular expression"
                ));
            }

            tracing::info!(
                "No top-level dependencies matched the regular expression, showing matching transitive dependencies"
            );
            transitive = true;
        }
    }

    let mut visited_pkgs = HashSet::new();
    let direct_dep_count = filtered_deps.len();

    for (index, pkg_name) in filtered_deps.iter().enumerate() {
        if !visited_pkgs.insert(pkg_name.clone()) {
            continue;
        }

        let last = index == direct_dep_count - 1;
        let mut symbol = "";
        if !transitive {
            symbol = if last {
                UTF8_SYMBOLS.ell
            } else {
                UTF8_SYMBOLS.tee
            };
        }
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
            print_dependency_node(
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

/// Prints a dependency tree node for a given package and its dependencies.
///
/// This function traverses the dependencies of a given package (`pkg`) and prints
/// them in a structured, tree-like format. If a dependency has already been
/// visited, it prevents infinite recursion by skipping that dependency.
///
/// # Arguments
///
/// * `handle` - A mutable reference to the standard output lock used for writing the output.
/// * `pkg` - A reference to the package whose dependencies are to be printed.
/// * `prefix` - A string used as a prefix for formatting tree branches.
/// * `dep_map` - A map of dependency names to their corresponding `Package` data.
/// * `visited_pkgs` - A mutable set of package names that have already been visited in the tree.
/// * `direct_deps` - A set of package names that should be highlighted.
///
/// # Returns
///
/// Returns `Ok(())` if the dependency tree for the given package is successfully printed,
/// or an error of type `miette::Result` if any operation fails during the process.
///
/// # Examples
///
/// Given a package with dependencies structured as a tree, calling this function
/// generates a visual tree-like output. For instance:
///
/// ```text
/// root
/// ├── dep1
/// │   └── subdep1
/// └── dep2
/// ```
///
/// Here `dep1` and `dep2` are direct dependencies of the `root`, and `subdep1` is a sub-dependency.
///
/// # Errors
///
/// This function can return an error if writing to the output stream fails.
fn print_dependency_node(
    handle: &mut StdoutLock,
    package: &Package,
    prefix: String,
    dep_map: &HashMap<String, Package>,
    visited_pkgs: &mut HashSet<String>,
    direct_deps: &HashSet<String>,
) -> miette::Result<()> {
    let dep_count = package.dependencies.len();
    for (index, dep_name) in package.dependencies.iter().enumerate() {
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
            print_dependency_node(handle, dep, new_prefix, dep_map, visited_pkgs, direct_deps)?;
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

/// Prints information about a package to the given output handle.
///
/// # Arguments
///
/// * `handle` - A mutable reference to the standard output lock used for writing the output.
/// * `prefix` - A string used as a prefix for formatting tree branches.
/// * `package` - A reference to the `Package` struct containing information about the package (e.g., name, version, source).
/// * `direct` - A boolean indicating if the package is a direct dependency. If `true`, the package name is formatted in bold green text.
/// * `visited` - A boolean indicating if the package has already been visited. If `true`, the printed output will include "(*)".
///
/// # Errors
/// This function can return an error if writing to the output stream fails.
pub fn print_package(
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

/// Prints an inverted hierarchical tree of dependencies to the provided output handle.
/// Dependencies with a name matching the passed regex are shown at the top level, with the packages that require them indented below.
///
/// # Arguments
///
/// * `handle` - A mutable lock on stdout for writing the output
/// * `inverted_dep_map` - Inverted map of packages with the `needed_by` field filled via [`build_reverse_dependency_map`]
/// * `direct_deps` - A set of package names that should be highlighted.
/// * `regex` - Regex pattern to filter which dependencies to display
///
/// # Returns
///
/// Returns `Ok(())` if the tree was printed successfully, or an error if the regex was invalid or no matches were found
pub fn print_inverted_dependency_tree(
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
                print_inverted_node(
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

/// Prints a dependency tree node for a given package and its dependents.
///
/// This function traverses the dependents of a given package (`pkg`) and prints
/// them in a structured, tree-like format. If a dependent has already been
/// visited, it prevents infinite recursion by skipping that dependent.
///
/// # Arguments
///
/// * `handle` - A mutable reference to the standard output lock used for writing the output.
/// * `package` - A reference to the package whose dependents are to be printed.
/// * `prefix` - A string used as a prefix for formatting tree branches.
/// * `inverted_dep_map` - A map of dependency names to their corresponding `Package` data.
/// * `visited_pkgs` - A mutable set of package names that have already been visited in the tree.
/// * `direct_deps` - A set of package names that should be highlighted.
///
/// # Returns
///
/// Returns `Ok(())` if the dependent tree for the given package is successfully printed,
/// or an error of type `miette::Result` if any operation fails during the process.
///
/// # Errors
///
/// This function can return an error if writing to the output stream fails.
fn print_inverted_node(
    handle: &mut StdoutLock,
    package: &Package,
    prefix: String,
    inverted_dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    visited_pkgs: &mut HashSet<String>,
) -> miette::Result<()> {
    let needed_count = package.needed_by.len();
    for (index, needed_name) in package.needed_by.iter().enumerate() {
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

                print_inverted_node(
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

/// Creates an inverted dependency graph by populating each package's `needed_by` field with the packages
/// that directly depend on it. Used to support the generation of reverse dependency trees.
///
/// This function is used to support generation of "reverse dependency" trees that show what packages depend
/// on a given package rather than what a package depends on.
///
/// # Arguments
///
/// * `dep_map` - A map of package names to `Package` objects representing the dependency graph
///
/// # Returns
///
/// A new dependency graph with `needed_by` fields populated to show reverse dependencies
pub fn build_reverse_dependency_map(
    dep_map: &HashMap<String, Package>,
) -> HashMap<String, Package> {
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
