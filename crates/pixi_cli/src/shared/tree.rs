//! Builds dependency trees and renders them as text or JSON.

use ahash::{HashSet, HashSetExt};
use console::Color;
use miette::{Context, IntoDiagnostic};
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;

/// Defines the source of a package. Global packages can only have Conda dependencies.
#[derive(Debug, Copy, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageSource {
    Conda,
    Pypi,
}

/// An edge from a package to one of its dependencies, optionally tagged with
/// the parent extras that activated it.
#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    /// Names of the parent's extras whose marker made this edge active. Empty
    /// for base (non-extra-gated) dependencies.
    pub via_extras: Vec<String>,
}

/// Represents a view of a Package with only the fields required for the tree visualization.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    /// `None` if the lock file doesn't record a version.
    pub version: Option<String>,
    pub dependencies: Vec<Dependency>,
    pub needed_by: Vec<String>,
    pub source: PackageSource,
}

/// A node of a rendered dependency tree.
///
/// Every package is expanded only once. Later occurrences are marked with
/// `is_deduplicated` and have no children.
#[derive(Debug, Serialize)]
pub struct TreeNode {
    pub name: String,
    /// `None` if the version is unknown, e.g. because the package isn't part
    /// of the locked environment.
    pub version: Option<String>,
    /// `None` if the package isn't part of the locked environment.
    pub kind: Option<PackageSource>,
    /// Whether the package is directly specified in the manifest.
    pub is_explicit: bool,
    pub via_extras: Vec<String>,
    pub is_deduplicated: bool,
    #[serde(flatten)]
    pub children: Children,
}

/// The children of a [`TreeNode`], serialized under a key that depends on
/// the direction of the tree.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Children {
    Dependencies(Vec<TreeNode>),
    RequiredBy(Vec<TreeNode>),
}

impl Children {
    fn nodes(&self) -> &[TreeNode] {
        match self {
            Children::Dependencies(nodes) | Children::RequiredBy(nodes) => nodes,
        }
    }
}

#[derive(Debug)]
pub enum Tree {
    /// Packages with their dependencies below them.
    Dependencies {
        roots: Vec<TreeNode>,
        /// The roots are transitive dependencies because no direct dependency
        /// matched the regex.
        transitive_roots: bool,
    },
    /// Packages with the packages that depend on them below them.
    Inverted { roots: Vec<TreeNode> },
}

impl Tree {
    pub fn roots(&self) -> &[TreeNode] {
        match self {
            Tree::Dependencies { roots, .. } | Tree::Inverted { roots } => roots,
        }
    }
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

/// Builds the tree requested on the command line and prints it as text or JSON.
pub fn print_requested_tree(
    dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    regex: Option<&str>,
    invert: bool,
    json: bool,
) -> miette::Result<()> {
    let tree = if invert {
        build_inverted_dependency_tree(&build_reverse_dependency_map(dep_map), direct_deps, regex)
            .wrap_err("Couldn't build the inverted dependency tree")?
    } else {
        build_dependency_tree(dep_map, direct_deps, regex)
            .wrap_err("Couldn't build the dependency tree")?
    };

    let mut handle = std::io::stdout().lock();
    if json {
        print_tree_json(&mut handle, &tree)
    } else {
        print_tree(&mut handle, &tree)
    }
}

/// Builds a tree with the direct dependencies as roots and their transitive
/// dependencies below them.
///
/// If a regex is given, only roots matching it are kept. If no direct
/// dependency matches, the matching transitive dependencies become the roots.
pub fn build_dependency_tree(
    dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    regex: Option<&str>,
) -> miette::Result<Tree> {
    let mut root_names = direct_deps.clone();
    let mut transitive_roots = false;

    if let Some(regex) = regex {
        let regex = Regex::new(regex)
            .into_diagnostic()
            .wrap_err("Invalid regular expression")?;

        root_names.retain(|p| regex.is_match(p));

        if root_names.is_empty() {
            root_names = dep_map
                .keys()
                .filter(|p| regex.is_match(p))
                .cloned()
                .collect();

            if root_names.is_empty() {
                return Err(miette::miette!(
                    "No dependencies matched the given regular expression"
                ));
            }

            tracing::info!(
                "No top-level dependencies matched the regular expression, showing matching transitive dependencies"
            );
            transitive_roots = true;
        }
    }

    let mut root_names: Vec<String> = root_names.into_iter().collect();
    root_names.sort();

    let mut visited = HashSet::new();
    let mut roots = Vec::new();
    for name in root_names {
        let Some(package) = dep_map.get(&name) else {
            continue;
        };
        // Roots that were already expanded below an earlier root are kept,
        // so that every root is listed.
        let is_deduplicated = !visited.insert(name);
        let children = if is_deduplicated {
            Vec::new()
        } else {
            dependency_nodes(package, dep_map, direct_deps, &mut visited)
        };
        roots.push(package_node(
            package,
            direct_deps,
            Vec::new(),
            is_deduplicated,
            Children::Dependencies(children),
        ));
    }

    Ok(Tree::Dependencies {
        roots,
        transitive_roots,
    })
}

fn dependency_nodes(
    package: &Package,
    dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    visited: &mut HashSet<String>,
) -> Vec<TreeNode> {
    let mut nodes = Vec::with_capacity(package.dependencies.len());
    for edge in &package.dependencies {
        let is_deduplicated = !visited.insert(edge.name.clone());
        let node = match dep_map.get(&edge.name) {
            Some(dep) => {
                let children = if is_deduplicated {
                    Vec::new()
                } else {
                    dependency_nodes(dep, dep_map, direct_deps, visited)
                };
                package_node(
                    dep,
                    direct_deps,
                    edge.via_extras.clone(),
                    is_deduplicated,
                    Children::Dependencies(children),
                )
            }
            None => TreeNode {
                name: edge.name.clone(),
                version: None,
                kind: None,
                is_explicit: false,
                via_extras: edge.via_extras.clone(),
                is_deduplicated,
                children: Children::Dependencies(Vec::new()),
            },
        };
        nodes.push(node);
    }
    nodes
}

/// Builds a tree with the packages matching `regex` as roots and the packages
/// that depend on them below them.
///
/// `inverted_dep_map` must have its `needed_by` fields filled via
/// [`build_reverse_dependency_map`].
pub fn build_inverted_dependency_tree(
    inverted_dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    regex: Option<&str>,
) -> miette::Result<Tree> {
    let regex = regex.ok_or_else(|| miette::miette!("The -i flag requires a package name."))?;
    let regex = Regex::new(regex)
        .into_diagnostic()
        .wrap_err("Invalid regular expression")?;

    let mut root_names: Vec<&String> = inverted_dep_map
        .keys()
        .filter(|p| regex.is_match(p))
        .collect();
    if root_names.is_empty() {
        return Err(miette::miette!(
            "Nothing depends on the given regular expression"
        ));
    }
    root_names.sort();

    let mut visited = HashSet::new();
    let roots = root_names
        .into_iter()
        .map(|name| {
            let package = &inverted_dep_map[name];
            let is_deduplicated = !visited.insert(name.clone());
            let children = if is_deduplicated {
                Vec::new()
            } else {
                dependent_nodes(package, inverted_dep_map, direct_deps, &mut visited)
            };
            package_node(
                package,
                direct_deps,
                Vec::new(),
                is_deduplicated,
                Children::RequiredBy(children),
            )
        })
        .collect();

    Ok(Tree::Inverted { roots })
}

fn dependent_nodes(
    package: &Package,
    inverted_dep_map: &HashMap<String, Package>,
    direct_deps: &HashSet<String>,
    visited: &mut HashSet<String>,
) -> Vec<TreeNode> {
    let mut nodes = Vec::with_capacity(package.needed_by.len());
    for name in &package.needed_by {
        let Some(dependent) = inverted_dep_map.get(name) else {
            continue;
        };
        let is_deduplicated = !visited.insert(name.clone());
        let children = if is_deduplicated {
            Vec::new()
        } else {
            dependent_nodes(dependent, inverted_dep_map, direct_deps, visited)
        };
        nodes.push(package_node(
            dependent,
            direct_deps,
            Vec::new(),
            is_deduplicated,
            Children::RequiredBy(children),
        ));
    }
    nodes
}

fn package_node(
    package: &Package,
    direct_deps: &HashSet<String>,
    via_extras: Vec<String>,
    is_deduplicated: bool,
    children: Children,
) -> TreeNode {
    TreeNode {
        name: package.name.clone(),
        version: package.version.clone(),
        kind: Some(package.source),
        is_explicit: direct_deps.contains(&package.name),
        via_extras,
        is_deduplicated,
        children,
    }
}

/// Prints the tree with box-drawing characters.
pub fn print_tree(handle: &mut impl Write, tree: &Tree) -> miette::Result<()> {
    match tree {
        Tree::Dependencies {
            roots,
            transitive_roots,
        } => {
            for (index, root) in roots.iter().enumerate() {
                let last = index == roots.len() - 1;
                let symbol = match (transitive_roots, last) {
                    (true, _) => "",
                    (false, true) => UTF8_SYMBOLS.ell,
                    (false, false) => UTF8_SYMBOLS.tee,
                };
                print_node(handle, &format!("{symbol} "), root)?;
                let prefix = if last {
                    UTF8_SYMBOLS.empty
                } else {
                    UTF8_SYMBOLS.down
                };
                print_children(handle, &format!("{prefix} "), root)?;
            }
        }
        Tree::Inverted { roots } => {
            for root in roots {
                print_node(handle, "\n", root)?;
                print_children(handle, "", root)?;
            }
        }
    }
    Ok(())
}

fn print_children(handle: &mut impl Write, prefix: &str, node: &TreeNode) -> miette::Result<()> {
    let children = node.children.nodes();
    for (index, child) in children.iter().enumerate() {
        let last = index == children.len() - 1;
        let (symbol, continuation) = if last {
            (UTF8_SYMBOLS.ell, UTF8_SYMBOLS.empty)
        } else {
            (UTF8_SYMBOLS.tee, UTF8_SYMBOLS.down)
        };
        print_node(handle, &format!("{prefix}{symbol} "), child)?;
        print_children(handle, &format!("{prefix}{continuation} "), child)?;
    }
    Ok(())
}

fn print_node(handle: &mut impl Write, prefix: &str, node: &TreeNode) -> miette::Result<()> {
    let extras_label = if node.via_extras.is_empty() {
        String::new()
    } else {
        format!("(extra: {})", node.via_extras.join(", "))
    };
    let name = if node.is_explicit {
        console::style(node.name.as_str()).fg(Color::Green).bold()
    } else {
        console::style(node.name.as_str())
    };
    let version = match (&node.version, node.kind) {
        (Some(version), _) => version.as_str(),
        (None, Some(_)) => "<unknown>",
        (None, None) => "",
    };
    let version = console::style(version).fg(match node.kind {
        Some(PackageSource::Pypi) => Color::Blue,
        Some(PackageSource::Conda) | None => Color::Yellow,
    });
    write_or_exit(writeln!(
        handle,
        "{}{} {} {}{}",
        prefix,
        name,
        version,
        console::style(extras_label).fg(Color::Cyan),
        if node.is_deduplicated { " (*)" } else { "" }
    ))
    .wrap_err("Failed to write package information")
}

/// Prints the roots of the tree as a JSON array.
pub fn print_tree_json(handle: &mut impl Write, tree: &Tree) -> miette::Result<()> {
    let json = serde_json::to_string_pretty(tree.roots())
        .expect("a tree only contains strings and booleans");
    write_or_exit(writeln!(handle, "{json}")).wrap_err("Failed to write the dependency tree")
}

/// Exits gracefully if the reader closed the pipe, e.g. `pixi tree | head`.
fn write_or_exit(result: std::io::Result<()>) -> miette::Result<()> {
    if let Err(e) = &result
        && e.kind() == std::io::ErrorKind::BrokenPipe
    {
        std::process::exit(0);
    }
    result.into_diagnostic()
}

/// Creates an inverted dependency graph by populating each package's `needed_by` field with the packages
/// that directly depend on it, sorted by name.
pub fn build_reverse_dependency_map(
    dep_map: &HashMap<String, Package>,
) -> HashMap<String, Package> {
    let mut inverted_deps = dep_map.clone();

    for pkg in dep_map.values() {
        for edge in pkg.dependencies.iter() {
            if let Some(idep) = inverted_deps.get_mut(&edge.name) {
                idep.needed_by.push(pkg.name.clone());
            }
        }
    }
    for pkg in inverted_deps.values_mut() {
        pkg.needed_by.sort();
    }

    inverted_deps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, source: PackageSource, deps: &[(&str, &[&str])]) -> Package {
        Package {
            name: name.to_string(),
            version: Some("1.0".to_string()),
            dependencies: deps
                .iter()
                .map(|(name, extras)| Dependency {
                    name: name.to_string(),
                    via_extras: extras.iter().map(ToString::to_string).collect(),
                })
                .collect(),
            needed_by: Vec::new(),
            source,
        }
    }

    /// `a -> b -> c -> a` is a cycle, `d` reaches `c` through an extra and
    /// `a` depends on `ghost`, which isn't locked.
    fn graph() -> (HashMap<String, Package>, HashSet<String>) {
        let packages = [
            package(
                "a",
                PackageSource::Conda,
                &[("b", &[]), ("c", &[]), ("ghost", &[])],
            ),
            package("b", PackageSource::Conda, &[("c", &[])]),
            package("c", PackageSource::Conda, &[("a", &[])]),
            package("d", PackageSource::Pypi, &[("c", &["x"])]),
        ];
        let dep_map = packages.into_iter().map(|p| (p.name.clone(), p)).collect();
        let direct_deps = ["a", "b", "d"].into_iter().map(String::from).collect();
        (dep_map, direct_deps)
    }

    fn render(tree: &Tree) -> String {
        console::set_colors_enabled(false);
        let mut text = Vec::new();
        print_tree(&mut text, tree).unwrap();
        let mut json = Vec::new();
        print_tree_json(&mut json, tree).unwrap();
        format!(
            "{}\n{}",
            String::from_utf8(text).unwrap(),
            String::from_utf8(json).unwrap()
        )
    }

    #[test]
    fn full_tree() {
        let (dep_map, direct_deps) = graph();
        let tree = build_dependency_tree(&dep_map, &direct_deps, None).unwrap();
        insta::assert_snapshot!(render(&tree));
    }

    #[test]
    fn regex_matching_direct_dependency() {
        let (dep_map, direct_deps) = graph();
        let tree = build_dependency_tree(&dep_map, &direct_deps, Some("^d$")).unwrap();
        insta::assert_snapshot!(render(&tree));
    }

    #[test]
    fn regex_falls_back_to_transitive_dependencies() {
        let (dep_map, direct_deps) = graph();
        let tree = build_dependency_tree(&dep_map, &direct_deps, Some("^c$")).unwrap();
        insta::assert_snapshot!(render(&tree));
    }

    #[test]
    fn regex_without_match() {
        let (dep_map, direct_deps) = graph();
        let error = build_dependency_tree(&dep_map, &direct_deps, Some("^nope$")).unwrap_err();
        assert_eq!(
            error.to_string(),
            "No dependencies matched the given regular expression"
        );
    }

    #[test]
    fn inverted_tree() {
        let (dep_map, direct_deps) = graph();
        let tree = build_inverted_dependency_tree(
            &build_reverse_dependency_map(&dep_map),
            &direct_deps,
            Some("^[bc]$"),
        )
        .unwrap();
        insta::assert_snapshot!(render(&tree));
    }

    #[test]
    fn inverted_tree_without_match() {
        let (dep_map, direct_deps) = graph();
        let error = build_inverted_dependency_tree(
            &build_reverse_dependency_map(&dep_map),
            &direct_deps,
            Some("^nope$"),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Nothing depends on the given regular expression"
        );
    }
}
