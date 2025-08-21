use itertools::Itertools;
use rattler_lock::LockedPackageRef;
use std::collections::{HashMap, HashSet, VecDeque};

/// A simplified representation of a package and its dependencies for efficient filtering.
#[derive(Clone, Debug)]
struct PackageNode {
    /// Name of the package
    pub name: String,
    /// The list of dependencies
    pub dependencies: Vec<String>,
}

impl<'a> From<LockedPackageRef<'a>> for PackageNode {
    /// Convert a LockedPackageRef to a PackageNode for efficient processing.
    fn from(package_ref: LockedPackageRef<'a>) -> Self {
        let name = package_ref.name().to_string();

        let dependency_names: Vec<String> = match package_ref {
            LockedPackageRef::Conda(conda_data) => {
                // Extract dependencies from conda data and parse as MatchSpec
                let depends = match conda_data {
                    rattler_lock::CondaPackageData::Binary(binary_data) => {
                        &binary_data.package_record.depends
                    }
                    rattler_lock::CondaPackageData::Source(source_data) => {
                        &source_data.package_record.depends
                    }
                };

                depends
                    .iter()
                    .filter_map(|dep_spec| {
                        // Parse as MatchSpec to get the package name
                        dep_spec
                            .parse::<rattler_conda_types::MatchSpec>()
                            .ok()
                            .and_then(|spec| spec.name.map(|name| name.as_normalized().to_string()))
                    })
                    .collect()
            }
            LockedPackageRef::Pypi(pypi_data, _env_data) => {
                // For PyPI, use the requirement directly to get the name
                pypi_data
                    .requires_dist
                    .iter()
                    .map(|req| req.name.to_string())
                    .collect()
            }
        };

        PackageNode {
            name,
            dependencies: dependency_names,
        }
    }
}

/// Filters packages using two skip modes and optional target selection.
///
/// - `skip_with_deps` (hard stop): drop the node and do not traverse through it.
///   Its subtree is only kept if reachable from another non-skipped root.
/// - `skip_direct` (passthrough): drop only the node; continue traversal through
///   its dependencies so they can still be kept if reachable.
///
/// When `target_package` is set, the target acts as the sole root of traversal.
/// The result returns both the packages to install/process and to ignore,
/// preserving the original package order.
pub struct InstallSubset<'a> {
    /// Packages to skip together with their dependencies (hard stop)
    skip_with_deps: &'a [String],
    /// Packages to skip directly but traverse through (passthrough)
    skip_direct: &'a [String],
    target_package: Option<&'a str>,
}

impl<'a> InstallSubset<'a> {
    /// Create a new package filter.
    pub fn new(
        skip_with_deps: &'a [String],
        skip_direct: &'a [String],
        target_package: Option<&'a str>,
    ) -> Self {
        Self {
            skip_with_deps,
            skip_direct,
            target_package,
        }
    }

    /// Filter packages based on skip and target settings with proper dependency handling.
    pub fn filter<'lock>(
        &self,
        packages: Option<impl IntoIterator<Item = LockedPackageRef<'lock>> + 'lock>,
    ) -> FilteredPackages<'lock> {
        // Handle None packages
        let Some(packages) = packages else {
            return FilteredPackages {
                install: Vec::new(),
                ignore: Vec::new(),
            };
        };

        let all_packages: Vec<_> = packages.into_iter().collect();

        match self.target_package {
            Some(target) => {
                // Target mode: Collect target + dependencies with skip short-circuiting
                let reach = Self::build_reachability(&all_packages);
                let required = reach.collect_target_dependencies(
                    target,
                    // This is the stop set, because we just short-circuit getting dependencies
                    self.skip_with_deps,
                    // This is the pasthrough set, because we are basically edge-joining
                    self.skip_direct,
                );

                // Map what we get back
                let to_process: Vec<_> = all_packages
                    .iter()
                    .filter(|pkg| required.contains(pkg.name()))
                    .copied()
                    .collect();
                let to_ignore: Vec<_> = all_packages
                    .iter()
                    .filter(|pkg| !required.contains(pkg.name()))
                    .copied()
                    .collect();
                FilteredPackages {
                    install: to_process,
                    ignore: to_ignore,
                }
            }
            None => {
                // Skip mode: Apply stop/passthrough rules from original roots
                self.filter_with_skips(&all_packages)
            }
        }
    }

    /// Filter out skip packages and only those dependencies that are no longer
    /// required by any remaining (non-skipped) package.
    fn filter_with_skips<'lock>(
        &self,
        all_packages: &[LockedPackageRef<'lock>],
    ) -> FilteredPackages<'lock> {
        if self.skip_with_deps.is_empty() && self.skip_direct.is_empty() {
            return FilteredPackages {
                install: all_packages.to_vec(),
                ignore: Vec::new(),
            };
        }

        // Compute the set of package names that remain required when the skip
        // packages are removed. We do this by walking the dependency graph
        // starting from every non-skipped package and never traversing through
        // skipped packages.
        let reach = Self::build_reachability(all_packages);
        let kept = reach.collect_reachable_from_non_skipped(self.skip_with_deps, self.skip_direct);
        let to_process: Vec<_> = all_packages
            .iter()
            .filter(|pkg| kept.contains(pkg.name()))
            .copied()
            .collect();
        let to_ignore: Vec<_> = all_packages
            .iter()
            .filter(|pkg| !kept.contains(pkg.name()))
            .copied()
            .collect();
        FilteredPackages {
            install: to_process,
            ignore: to_ignore,
        }
    }

    /// Build a reachability analyzer for a set of packages.
    fn build_reachability(all_packages: &[LockedPackageRef<'_>]) -> PackageReachability {
        let nodes: Vec<PackageNode> = all_packages.iter().copied().map(Into::into).collect();
        PackageReachability::new(nodes)
    }
}

/// Result of applying an InstallSubset over a package set.
#[derive(Default)]
pub struct FilteredPackages<'lock> {
    pub install: Vec<LockedPackageRef<'lock>>,
    pub ignore: Vec<LockedPackageRef<'lock>>,
}

/// Collects reachability over the package graph.
///
/// Traversal rules use two skip sets:
/// - stop_set: do not include node and do not traverse its dependencies.
/// - passthrough_set: do not include node but DO traverse its dependencies.
struct PackageReachability {
    /// Flattened nodes for fast traversal.
    nodes: Vec<PackageNode>,
    /// Map package name -> index into `nodes`.
    name_to_index: HashMap<String, usize>,
}

impl PackageReachability {
    /// Build a collector from a list of nodes.
    pub(crate) fn new(nodes: Vec<PackageNode>) -> Self {
        let name_to_index: HashMap<String, usize> = nodes
            .iter()
            .enumerate()
            .map(|(idx, node)| (node.name.clone(), idx))
            .collect();

        Self {
            nodes,
            name_to_index,
        }
    }

    /// Collect target package and all its dependencies, excluding specified packages.
    pub(crate) fn collect_target_dependencies(
        &self,
        target: &str,
        stop_set: &[String],
        passthrough_set: &[String],
    ) -> HashSet<String> {
        // BFS over target's dependency tree with exclusions
        let mut included: HashSet<String> = HashSet::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue = VecDeque::new();
        let stop_set: HashSet<&str> = stop_set.iter().map(|s| s.as_str()).collect();
        let passthrough_set: HashSet<&str> = passthrough_set.iter().map(|s| s.as_str()).collect();
        queue.push_back(target.to_string());

        while let Some(package_name) = queue.pop_front() {
            // Stop entirely at stop_set
            if stop_set.contains(package_name.as_str()) {
                continue;
            }

            if !seen.insert(package_name.clone()) {
                continue;
            }

            // Include this node unless it is passthrough
            if !passthrough_set.contains(package_name.as_str()) {
                included.insert(package_name.clone());
            }

            if let Some(&idx) = self.name_to_index.get(&package_name) {
                for dep_name in self.nodes[idx].dependencies.iter() {
                    // Always traverse into children unless they are in stop_set,
                    // even when the current node is passthrough.
                    if self.name_to_index.contains_key(dep_name) {
                        queue.push_back(dep_name.clone());
                    }
                }
            }
        }

        included
    }

    /// Compute the set of packages that should be kept when skipping a set of
    /// packages. This keeps any package that is reachable from at least one
    /// non-skipped package without traversing through a skipped package.
    pub(crate) fn collect_reachable_from_non_skipped(
        &self,
        stop_set: &[String],
        passthrough_set: &[String],
    ) -> HashSet<String> {
        // Compute indegree to determine original roots.
        // indegree == 0 are the initial roots.
        let mut indegree: HashMap<&str, usize> = self
            .name_to_index
            .keys()
            .map(|k| (k.as_str(), 0usize))
            .collect();
        for node in &self.nodes {
            for dep in &node.dependencies {
                if let Some(entry) = indegree.get_mut(dep.as_str()) {
                    *entry += 1;
                }
            }
        }
        let roots = indegree
            .into_iter()
            .filter_map(|(name, deg)| {
                if deg == 0 {
                    Some(name.to_string())
                } else {
                    None
                }
            })
            .collect_vec();

        let stop_set: HashSet<&str> = stop_set.iter().map(|s| s.as_str()).collect();
        let passthrough_set: HashSet<&str> = passthrough_set.iter().map(|s| s.as_str()).collect();

        let mut kept = HashSet::new();
        let mut queue = VecDeque::new();

        // Initialize the queue with all non-skipped original roots.
        for name in &roots {
            if !stop_set.contains(name.as_str()) {
                queue.push_back(name.clone());
            }
        }

        while let Some(name) = queue.pop_front() {
            // Never include skipped packages.
            if stop_set.contains(name.as_str()) {
                continue;
            }

            // Insert and continue traversal if newly seen.
            if !kept.insert(name.clone()) {
                continue;
            }

            if let Some(&idx) = self.name_to_index.get(&name) {
                for dep in &self.nodes[idx].dependencies {
                    // Do not traverse into stop_set; passthrough happens by excluding at the end
                    if !stop_set.contains(dep.as_str()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        // Remove passthrough nodes from the kept set, but leave traversal effect
        kept.retain(|name| !passthrough_set.contains(name.as_str()));
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, deps: &[&str]) -> PackageNode {
        PackageNode {
            name: name.to_string(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
        }
    }

    // Graph: A -> B -> C <- F
    fn graph_a_b_c_f() -> PackageReachability {
        let nodes = vec![
            node("A", &["B"]),
            node("B", &["C"]),
            node("C", &[]),
            node("F", &["C"]),
        ];
        PackageReachability::new(nodes)
    }

    #[test]
    fn reachable_with_skip_a_keeps_c_via_f() {
        let dc = graph_a_b_c_f();
        let kept = dc.collect_reachable_from_non_skipped(&["A".to_string()], &[]);
        // Roots: A, F. With A skipped, traversal from F reaches F and C.
        assert!(kept.contains("F"));
        assert!(kept.contains("C"));
        assert!(!kept.contains("B"));
        assert!(!kept.contains("A"));
    }

    #[test]
    fn reachable_with_skip_a_and_f_drops_c() {
        let dc = graph_a_b_c_f();
        let kept = dc.collect_reachable_from_non_skipped(&["A".to_string(), "F".to_string()], &[]);
        // Both roots skipped, nothing should remain reachable.
        assert!(kept.is_empty());
    }

    #[test]
    fn target_with_skip_short_circuits_dependencies() {
        // A -> B -> C <- D
        let nodes = vec![
            node("A", &["B"]),
            node("B", &["C"]),
            node("C", &[]),
            node("D", &["C"]),
        ];
        let dc = PackageReachability::new(nodes);
        let required = dc.collect_target_dependencies("A", &["B".to_string()], &[]);

        assert!(required.contains("A"));
        assert!(!required.contains("B"));
        assert!(!required.contains("C"));
        assert!(!required.contains("D"));
    }

    #[test]
    fn reachable_with_skip_direct_passthrough() {
        // A -> B -> C
        let nodes = vec![node("A", &["B"]), node("B", &["C"]), node("C", &[])];
        let dc = PackageReachability::new(nodes);
        let kept = dc.collect_reachable_from_non_skipped(&[], &["B".to_string()]);
        assert!(kept.contains("A"));
        assert!(kept.contains("C"));
        assert!(!kept.contains("B"));
    }

    #[test]
    fn target_with_skip_direct_passthrough() {
        // A -> B -> C <- D, target=A, skip_direct=B keeps A and C
        let nodes = vec![
            node("A", &["B"]),
            node("B", &["C"]),
            node("C", &[]),
            node("D", &["C"]),
        ];
        let dc = PackageReachability::new(nodes);
        let required = dc.collect_target_dependencies("A", &[], &["B".to_string()]);
        assert!(required.contains("A"));
        assert!(!required.contains("B"));
        assert!(required.contains("C"));
    }

    #[test]
    fn diamond_graph_retains_shared_dep() {
        // A -> B, A -> C, B -> D, C -> D
        let nodes = vec![
            node("A", &["B", "C"]),
            node("B", &["D"]),
            node("C", &["D"]),
            node("D", &[]),
        ];
        let dc = PackageReachability::new(nodes);
        let kept = dc.collect_reachable_from_non_skipped(&[], &[]);
        assert!(
            kept.contains("A") && kept.contains("B") && kept.contains("C") && kept.contains("D")
        );
    }

    #[test]
    fn with_deps_overrides_direct_when_both_present() {
        // A -> B -> C; if B in both stop and passthrough, stop wins and C
        // should only be kept if reachable from another root (it is not).
        let nodes = vec![node("A", &["B"]), node("B", &["C"]), node("C", &[])];
        let dc = PackageReachability::new(nodes);
        let kept = dc.collect_reachable_from_non_skipped(&["B".to_string()], &["B".to_string()]);
        assert!(kept.contains("A"));
        assert!(!kept.contains("B"));
        assert!(!kept.contains("C"));
    }
}
