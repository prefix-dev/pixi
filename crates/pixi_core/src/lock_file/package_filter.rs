use std::collections::{HashMap, HashSet};
use rattler_lock::LockedPackageRef;

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

    /// Filter packages based on skip and target settings.
    ///
    /// If target_package is Some, first selects target + dependencies, then applies skip filter.
    /// If target_package is None, applies skip filter to all packages (existing behavior).
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
                // Scenario 2: Select target + deps, then apply skip to that subset
                let target_and_deps = self.collect_target_dependencies(&all_packages, target);
                self.apply_skip_filter(target_and_deps)
            }
            None => {
                // Scenario 1: Apply skip to all packages (existing behavior)
                self.apply_skip_filter(all_packages)
            }
        }
    }

    /// Collect target package and all its dependencies.
    fn collect_target_dependencies<'lock>(
        &self,
        all_packages: &[LockedPackageRef<'lock>],
        target: &str,
    ) -> Vec<LockedPackageRef<'lock>> {
        // Create a map for efficient package lookup by name
        let package_map: HashMap<String, LockedPackageRef<'lock>> = all_packages
            .iter()
            .map(|pkg| (pkg.name().to_string(), *pkg))
            .collect();

        // Collect target + dependencies recursively
        let mut required_packages = HashSet::new();
        self.collect_dependencies_recursive(&package_map, target, &mut required_packages);

        // Return packages that are in required set
        all_packages
            .iter()
            .filter(|pkg| required_packages.contains(&pkg.name().to_string()))
            .copied()
            .collect()
    }

    /// Recursively collect dependencies from lock file data.
    fn collect_dependencies_recursive(
        &self,
        package_map: &HashMap<String, LockedPackageRef<'_>>,
        package_name: &str,
        collected: &mut HashSet<String>,
    ) {
        // If already processed, skip to avoid infinite recursion
        if !collected.insert(package_name.to_string()) {
            return;
        }

        // Get dependencies for this package directly from lock file data
        if let Some(package) = package_map.get(package_name) {
            let dependency_names: Vec<String> = match package {
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
                            dep_spec.parse::<rattler_conda_types::MatchSpec>()
                                .ok()
                                .and_then(|spec| spec.name.map(|name| name.as_str().to_string()))
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

            // Process each dependency name
            for dep_name in dependency_names {
                // Only process dependencies that exist in our package set
                if package_map.contains_key(&dep_name) {
                    self.collect_dependencies_recursive(package_map, &dep_name, collected);
                }
            }
        }
    }

    /// Apply skip filtering to a set of packages.
    fn apply_skip_filter<'lock>(
        &self,
        packages: Vec<LockedPackageRef<'lock>>,
    ) -> Vec<LockedPackageRef<'lock>> {
        if self.skip_packages.is_empty() {
            packages
        } else {
            packages
                .into_iter()
                .filter(|pkg| !self.skip_packages.contains(&pkg.name().to_string()))
                .collect()
        }
    }
}