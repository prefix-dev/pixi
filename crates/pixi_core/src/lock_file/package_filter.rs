use rattler_lock::LockedPackageRef;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};

/// A simplified representation of a package and its dependencies for efficient filtering.
#[derive(Clone, Debug)]
pub struct PackageNode<'a> {
    pub name: Cow<'a, str>,
    pub dependencies: Cow<'a, [String]>,
}

impl<'a> PackageNode<'a> {
    /// Convert a LockedPackageRef to a PackageNode for efficient processing.
    pub fn from_locked_package_ref(package_ref: &'a LockedPackageRef<'a>) -> Self {
        let name = Cow::Owned(package_ref.name().to_string());

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
            dependencies: Cow::Owned(dependency_names),
        }
    }
}

/// A filter for packages that can handle both skipping packages and selecting
/// a target package with its dependencies.
pub struct PackageFilter<'a> {
    skip_packages: &'a [String],
    target_package: Option<&'a str>,
}

impl<'a> PackageFilter<'a> {
    /// Create a new package filter.
    pub fn new(skip_packages: &'a [String], target_package: Option<&'a str>) -> Self {
        Self {
            skip_packages,
            target_package,
        }
    }

    /// Filter packages based on skip and target settings with proper dependency handling.
    pub fn filter<'lock>(
        &self,
        packages: Option<impl IntoIterator<Item = LockedPackageRef<'lock>> + 'lock>,
    ) -> Vec<LockedPackageRef<'lock>> {
        // Handle None packages
        let Some(packages) = packages else {
            return Vec::new();
        };

        let all_packages: Vec<_> = packages.into_iter().collect();

        match self.target_package {
            Some(target) => {
                // Target mode: Collect target + dependencies with skip short-circuiting
                DependencyCollector::collect_target_dependencies(
                    &all_packages,
                    target,
                    self.skip_packages,
                )
            }
            None => {
                // Skip mode: Collect each skip package + dependencies, then invert
                self.filter_skip_with_dependencies(&all_packages)
            }
        }
    }

    /// Filter out skip packages and all their dependencies from the full set.
    fn filter_skip_with_dependencies<'lock>(
        &self,
        all_packages: &[LockedPackageRef<'lock>],
    ) -> Vec<LockedPackageRef<'lock>> {
        if self.skip_packages.is_empty() {
            return all_packages.to_vec();
        }

        // Convert to PackageNodes for efficient processing
        let package_nodes: Vec<PackageNode<'_>> = all_packages
            .iter()
            .map(PackageNode::from_locked_package_ref)
            .collect();

        // Collect all packages to skip (skip packages + their dependencies)
        let mut packages_to_skip = HashSet::new();
        for skip_package in self.skip_packages {
            let skip_deps =
                DependencyCollector::collect_dependencies(&package_nodes, skip_package, &[]);
            packages_to_skip.extend(skip_deps);
        }

        // Return packages that are NOT in the skip set
        all_packages
            .iter()
            .filter(|pkg| !packages_to_skip.contains(&pkg.name()))
            .copied()
            .collect()
    }
}

/// Helper for collecting package dependencies from lock file data.
struct DependencyCollector;

impl DependencyCollector {
    /// Collect a package and all its dependencies using a queue-based approach with exclusions.
    /// Returns a set of package names that includes the starting package and all its transitive dependencies,
    /// but stops traversing when encountering excluded packages (short-circuits to avoid including their deps).
    fn collect_dependencies(
        package_nodes: &[PackageNode<'_>],
        from_package: &str,
        exclude_packages: &[String],
    ) -> HashSet<String> {
        // Create a map for efficient package lookup by name
        let package_map: HashMap<String, &PackageNode<'_>> = package_nodes
            .iter()
            .map(|node| (node.name.to_string(), node))
            .collect();

        let mut collected = HashSet::new();
        let mut queue = VecDeque::new();

        // Start with the from package
        queue.push_back(from_package.to_string());

        while let Some(package_name) = queue.pop_front() {
            // Short-circuit: if this package is excluded, don't include it or its dependencies
            if exclude_packages.contains(&package_name) {
                continue;
            }

            // If already processed, skip to avoid infinite loops
            if !collected.insert(package_name.clone()) {
                continue;
            }

            // Get dependencies for this package from PackageNode
            if let Some(package_node) = package_map.get(&package_name) {
                // Add dependencies to queue for processing
                for dep_name in package_node.dependencies.iter() {
                    // Only process dependencies that exist in our package set
                    if package_map.contains_key(dep_name) && !collected.contains(dep_name) {
                        queue.push_back(dep_name.clone());
                    }
                }
            }
        }

        collected
    }

    /// Collect target package and all its dependencies from a package list, excluding specified packages.
    fn collect_target_dependencies<'lock>(
        all_packages: &[LockedPackageRef<'lock>],
        target: &str,
        exclude_packages: &[String],
    ) -> Vec<LockedPackageRef<'lock>> {
        // Convert to PackageNodes for efficient processing
        let package_nodes: Vec<PackageNode<'_>> = all_packages
            .iter()
            .map(PackageNode::from_locked_package_ref)
            .collect();

        // Collect target + dependencies with exclusions
        let required_packages =
            Self::collect_dependencies(&package_nodes, target, exclude_packages);

        // Return packages that are in required set
        all_packages
            .iter()
            .filter(|pkg| required_packages.contains(&pkg.name().to_string()))
            .copied()
            .collect()
    }
}
