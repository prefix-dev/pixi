use itertools::Itertools;
use miette::Diagnostic;
use rattler_lock::LockedPackageRef;
use std::collections::{HashMap, HashSet, VecDeque};

/// A simplified representation of a package and its dependencies for efficient filtering.
///
/// Note: dependency names here are already normalized package names. We only
/// capture names (not version constraints) because the reachability algorithms
/// work purely on the structure of the dependency graph.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PackageSource {
    Conda,
    Pypi,
}

/// A simplified representation of a package and its dependencies for efficient filtering.
///
/// Note: dependency names here are already normalized package names. We only
/// capture names (not version constraints) because the reachability algorithms
/// work purely on the structure of the dependency graph.
#[derive(Clone, Debug)]
struct PackageNode {
    /// Name of the package
    pub name: String,
    /// The list of dependencies
    pub dependencies: Vec<String>,
    /// Source of the package (Conda or PyPI)
    pub source: PackageSource,
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
            source: match package_ref {
                LockedPackageRef::Conda(_) => PackageSource::Conda,
                LockedPackageRef::Pypi(_, _) => PackageSource::Pypi,
            },
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
/// When `target_packages` are set, the targets act as the roots of traversal.
/// The result returns both the packages to install/process and to ignore,
/// preserving the original package order.
///
/// ## Note for implementers
/// One thing that I had is that when creating this code, it was tripping me up that `skip_with_deps` maps into the
/// `stop_set` while the `skip_direct` maps into the passthrough set. Intuitively it feels like the opposite.
/// To make sense of this, first consider that the algorithms is interested in finding what can still be reached under new constraints.
///
/// So while the user is interested in what to *skip*, we are interested in what we can still *reach*. Thats why things are the inverse.
/// 1. Hence, think of the `skip_with_deps` as pruning parts of the the tree so the place for it is the `stop_set`.
/// 2. And think of `skip_direct` as edge joining two nodes, basically ignoring the skipped node, so the place for it is the `passthrough_set`.
/// 3. Finally, think of the `target` node as zooming into the tree selecting that nodes and its dependencies and basically ignoring the rest.
pub struct InstallSubset<'a> {
    /// Packages to skip together with their dependencies (hard stop)
    skip_with_deps: &'a [String],
    /// Packages to skip directly but traverse through (passthrough)
    skip_direct: &'a [String],
    /// Which packages should be targeted directly (zooming in); empty means no targeting
    target_packages: &'a [String],
}

#[derive(thiserror::Error, Debug, Diagnostic)]
pub enum InstallSubsetError {
    #[error("the following `--only` packages do not exist: {}", .0.iter().map(|s| format!("'{}'", s)).join(", "))]
    #[diagnostic(help("try finding the correct package with `pixi list`"))]
    TargetPackagesDoNotExist(Vec<String>),
}

impl<'a> InstallSubset<'a> {
    /// Create a new package filter.
    pub fn new(
        skip_with_deps: &'a [String],
        skip_direct: &'a [String],
        target_packages: &'a [String],
    ) -> Self {
        Self {
            skip_with_deps,
            skip_direct,
            target_packages,
        }
    }

    /// Filter packages based on skip and target settings with proper dependency handling.
    ///
    /// Both traversals run in O(V+E) time on the constructed graph.
    /// Algorithm overview:
    /// - Convert the input packages to a compact graph representation.
    /// - If `target_packages` are provided: run a BFS starting at those targets,
    ///   short-circuiting at `skip_with_deps` and not including nodes in `skip_direct`.
    /// - Else (skip-mode): find original graph roots (indegree 0) and run a BFS
    ///   from those roots, again not traversing into `skip_with_deps`, and exclude
    ///   nodes in `skip_direct` from the final result.
    pub fn filter<'lock>(
        &self,
        packages: Option<impl IntoIterator<Item = LockedPackageRef<'lock>> + 'lock>,
    ) -> Result<FilteredPackages<'lock>, InstallSubsetError> {
        // Handle None packages
        let Some(packages) = packages else {
            return Ok(FilteredPackages::new(Vec::new(), Vec::new()));
        };

        let all_packages: Vec<_> = packages.into_iter().collect();

        // Check if any packages do not match
        let mut non_matched_targets: HashSet<_> =
            self.target_packages.iter().map(AsRef::as_ref).collect();
        for package in &all_packages {
            if non_matched_targets.contains(package.name()) {
                non_matched_targets.remove(package.name());
            }
        }
        if !non_matched_targets.is_empty() {
            return Err(InstallSubsetError::TargetPackagesDoNotExist(
                non_matched_targets
                    .iter()
                    .map(ToString::to_string)
                    .collect_vec(),
            ));
        }

        let filtered_packages = if !self.target_packages.is_empty() {
            // Target mode: Collect targets + dependencies with skip short-circuiting
            let reach = Self::build_reachability(&all_packages);
            let required = reach.collect_targets_dependencies(
                self.target_packages,
                // This is the stop set, because we just short-circuit getting dependencies
                self.skip_with_deps,
                // This is the passthrough set, because we are basically edge-joining
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
            FilteredPackages::new(to_process, to_ignore)
        } else {
            // Skip mode: Apply stop/passthrough rules from original roots
            self.filter_with_skips(&all_packages)
        };

        Ok(filtered_packages)
    }

    /// Filter out skip packages and only those dependencies that are no longer
    /// required by any remaining (non-skipped) package.
    fn filter_with_skips<'lock>(
        &self,
        all_packages: &[LockedPackageRef<'lock>],
    ) -> FilteredPackages<'lock> {
        if self.skip_with_deps.is_empty() && self.skip_direct.is_empty() {
            return FilteredPackages::new(all_packages.to_vec(), Vec::new());
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
        FilteredPackages::new(to_process, to_ignore)
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

impl<'lock> FilteredPackages<'lock> {
    pub fn new(
        install: Vec<LockedPackageRef<'lock>>,
        ignore: Vec<LockedPackageRef<'lock>>,
    ) -> Self {
        FilteredPackages { install, ignore }
    }
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
    /// Adjacency list of dependency indices for fast traversal.
    edges: Vec<Vec<usize>>,
}

impl PackageReachability {
    /// Build a collector from a list of nodes.
    ///
    /// This constructs:
    /// - `name_to_index` for O(1) name→index lookups.
    /// - `edges` adjacency lists (indices only) for tight traversal loops without
    ///   repeated string hashing or allocation.
    pub(crate) fn new(nodes: Vec<PackageNode>) -> Self {
        let name_to_index: HashMap<String, usize> = nodes
            .iter()
            .enumerate()
            .map(|(idx, node)| (node.name.clone(), idx))
            .collect();

        // Build compact adjacency list by resolving dependency names to indices
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
        for (idx, node) in nodes.iter().enumerate() {
            let deps = node
                .dependencies
                .iter()
                .filter_map(|name| name_to_index.get(name).copied())
                .collect();
            edges[idx] = deps;
        }

        Self {
            nodes,
            name_to_index,
            edges,
        }
    }

    /// If the current required set contains any PyPI package, ensure a Conda
    /// `python` package is also included when one exists in the graph.
    fn augment_with_python_if_pypi(&self, required: &mut HashSet<String>) {
        let has_pypi_included = self
            .nodes
            .iter()
            .any(|n| matches!(n.source, PackageSource::Pypi) && required.contains(&n.name));

        if has_pypi_included {
            let has_conda_python = self
                .nodes
                .iter()
                .any(|n| matches!(n.source, PackageSource::Conda) && n.name.as_str() == "python");
            if has_conda_python {
                required.insert("python".to_string());
            }
        }
    }

    /// Collect target package(s) and all their dependencies, excluding specified packages.
    /// Collect all packages reachable from `targets` under skip rules.
    ///
    /// Semantics:
    /// - `stop_set` (skip-with-deps): do not include the node and do not traverse
    ///   into its dependencies.
    /// - `passthrough_set` (skip-direct): do not include the node, but continue
    ///   traversal into its dependencies.
    ///
    /// Implementation details:
    /// - Uses index-based BFS over `edges` with boolean bitsets for membership tests
    pub(crate) fn collect_targets_dependencies(
        &self,
        targets: &[String],
        stop_set: &[String],
        passthrough_set: &[String],
    ) -> HashSet<String> {
        // Resolve sets to boolean index maps for fast membership checks
        let mut stop_idx = vec![false; self.nodes.len()];
        for name in stop_set {
            if let Some(&i) = self.name_to_index.get(name) {
                stop_idx[i] = true;
            }
        }
        // Do the same for the passthrough set
        let mut pass_idx = vec![false; self.nodes.len()];
        for name in passthrough_set {
            if let Some(&i) = self.name_to_index.get(name) {
                pass_idx[i] = true;
            }
        }

        // BFS over targets' dependency trees with exclusions
        let mut included = vec![false; self.nodes.len()];
        let mut seen = vec![false; self.nodes.len()];
        let mut queue = VecDeque::new();

        // Start from all provided targets that exist; if none exist, nothing is required.
        for target in targets {
            if let Some(&start) = self.name_to_index.get(target) {
                queue.push_back(start);
            }
        }
        if queue.is_empty() {
            return HashSet::new();
        }

        while let Some(idx) = queue.pop_front() {
            // Do not include or traverse nodes in the stop set.
            if stop_idx[idx] {
                continue;
            }
            if std::mem::replace(&mut seen[idx], true) {
                continue;
            }
            // Include current node unless it is marked passthrough.
            if !pass_idx[idx] {
                included[idx] = true;
            }
            for &dep_idx in &self.edges[idx] {
                // Always traverse into children unless they are stopped.
                if !stop_idx[dep_idx] {
                    queue.push_back(dep_idx);
                }
            }
        }

        // Materialize result names, then augment for python if needed
        let mut result = HashSet::with_capacity(self.nodes.len());
        for (i, inc) in included.iter().enumerate() {
            if *inc {
                result.insert(self.nodes[i].name.clone());
            }
        }
        self.augment_with_python_if_pypi(&mut result);
        result
    }

    /// Compute the set of packages that should be kept when skipping a set of
    /// packages. This keeps any package that is reachable from at least one
    /// non-skipped package without traversing through a skipped package.
    /// Compute the set of packages that remain required when skipping packages.
    ///
    /// Approach:
    /// - Determine original roots as nodes with indegree 0. These represent the
    ///   starting points of the environment before skips.
    /// - BFS from those roots, never traversing into `stop_set` nodes.
    /// - Mark nodes as kept unless they are in `passthrough_set` (skip-direct).
    /// - Complexity: O(V+E) for the traversal and indegree computation.
    pub(crate) fn collect_reachable_from_non_skipped(
        &self,
        stop_set: &[String],
        passthrough_set: &[String],
    ) -> HashSet<String> {
        // Resolve sets to boolean index maps
        let mut stop_idx = vec![false; self.nodes.len()];
        for name in stop_set {
            if let Some(&i) = self.name_to_index.get(name) {
                stop_idx[i] = true;
            }
        }
        let mut pass_idx = vec![false; self.nodes.len()];
        for name in passthrough_set {
            if let Some(&i) = self.name_to_index.get(name) {
                pass_idx[i] = true;
            }
        }

        // Compute indegree to determine original roots (nodes with indegree 0).
        // We do this on the full graph: skip rules affect traversal/inclusion,
        // not what counts as a structural root.
        let mut indegree = vec![0usize; self.nodes.len()];
        for deps in &self.edges {
            for &dep in deps {
                indegree[dep] = indegree[dep].saturating_add(1);
            }
        }
        let roots: Vec<usize> = indegree
            .iter()
            .enumerate()
            .filter_map(|(i, &deg)| if deg == 0 { Some(i) } else { None })
            .collect();

        let mut kept = vec![false; self.nodes.len()];
        let mut seen = vec![false; self.nodes.len()];
        let mut queue = VecDeque::new();

        // Initialize the queue with all non-skipped original roots.
        for &root in &roots {
            if !stop_idx[root] {
                queue.push_back(root);
            }
        }

        while let Some(idx) = queue.pop_front() {
            // Never include or traverse stopped nodes.
            if stop_idx[idx] {
                continue;
            }
            if std::mem::replace(&mut seen[idx], true) {
                continue;
            }

            // Include unless marked passthrough.
            if !pass_idx[idx] {
                kept[idx] = true;
            }

            for &dep in &self.edges[idx] {
                // Do not traverse into stop_set; passthrough happens by excluding at the end
                if !stop_idx[dep] {
                    queue.push_back(dep);
                }
            }
        }

        // Remove passthrough nodes from the kept set, but leave traversal effect
        let mut result = HashSet::with_capacity(self.nodes.len());
        for (i, &is_kept) in kept.iter().enumerate() {
            if is_kept && !pass_idx[i] {
                result.insert(self.nodes[i].name.clone());
            }
        }
        self.augment_with_python_if_pypi(&mut result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, deps: &[&str]) -> PackageNode {
        // Default test helper creates Conda nodes
        PackageNode {
            name: name.to_string(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            source: PackageSource::Conda,
        }
    }

    fn node_pypi(name: &str, deps: &[&str]) -> PackageNode {
        PackageNode {
            name: name.to_string(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            source: PackageSource::Pypi,
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
        let required = dc.collect_targets_dependencies(&["A".to_string()], &["B".to_string()], &[]);

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
        let required = dc.collect_targets_dependencies(&["A".to_string()], &[], &["B".to_string()]);
        assert!(required.contains("A"));
        assert!(!required.contains("B"));
        assert!(required.contains("C"));
    }

    #[test]
    fn multiple_targets_union_dependencies() {
        // Graph: A -> X, B -> Y, X -> Z, Y -> Z
        // Targets: A and B should include A, B, X, Y, Z
        let nodes = vec![
            node("A", &["X"]),
            node("B", &["Y"]),
            node("X", &["Z"]),
            node("Y", &["Z"]),
            node("Z", &[]),
        ];
        let dc = PackageReachability::new(nodes);
        let required =
            dc.collect_targets_dependencies(&["A".to_string(), "B".to_string()], &[], &[]);
        for n in ["A", "B", "X", "Y", "Z"] {
            assert!(required.contains(n), "expected to contain {}", n);
        }
    }

    #[test]
    fn multiple_targets_respect_passthrough_skips() {
        // A -> B, C -> D; skip_direct = B
        // Targets A, C => include A, C, D; exclude B
        let nodes = vec![
            node("A", &["B"]),
            node("B", &[]),
            node("C", &["D"]),
            node("D", &[]),
        ];
        let dc = PackageReachability::new(nodes);
        let required = dc.collect_targets_dependencies(
            &["A".to_string(), "C".to_string()],
            &[],
            &["B".to_string()],
        );
        assert!(required.contains("A"));
        assert!(required.contains("C"));
        assert!(required.contains("D"));
        assert!(!required.contains("B"));
    }

    #[test]
    fn multiple_targets_respect_stop_skips() {
        // A -> B, C -> D; skip_with_deps = B
        // Targets A, C => include A (but not B), include C and D
        let nodes = vec![
            node("A", &["B"]),
            node("B", &[]),
            node("C", &["D"]),
            node("D", &[]),
        ];
        let dc = PackageReachability::new(nodes);
        let required = dc.collect_targets_dependencies(
            &["A".to_string(), "C".to_string()],
            &["B".to_string()],
            &[],
        );
        assert!(required.contains("A"));
        assert!(required.contains("C"));
        assert!(required.contains("D"));
        assert!(!required.contains("B"));
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
        let kept = dc.collect_reachable_from_non_skipped(&["B".to_string()], &[]);
        assert!(
            kept.contains("A") && !kept.contains("B") && kept.contains("C") && kept.contains("D")
        );
    }

    #[test]
    fn skipped_is_same_as_target() {
        // A -> B, A -> C, we both target and skip A, in our current implementation the skipped takes precedence over only
        let nodes = vec![node("A", &["B", "C"]), node("B", &[]), node("C", &[])];

        let dc = PackageReachability::new(nodes);
        let kept = dc.collect_targets_dependencies(&["A".to_string()], &["A".to_string()], &[]);
        assert!(kept.is_empty());
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

    #[test]
    fn adds_python_when_pypi_is_included() {
        // Graph with no edges, but includes a PyPI package and a Conda python.
        let nodes = vec![node("python", &[]), node_pypi("requests", &[])];
        let dc = PackageReachability::new(nodes);

        // Simulate result set that includes a PyPI package
        let mut required: HashSet<String> = ["requests".to_string()].into_iter().collect();
        dc.augment_with_python_if_pypi(&mut required);

        assert!(required.contains("python"));
        assert!(required.contains("requests"));
    }
}
