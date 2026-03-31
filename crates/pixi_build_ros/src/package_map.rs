//! Package mapping resolution between ROS and conda package names.
//!
//! Loads `robostack.yaml` and user-provided mappings, resolves ROS dependency
//! names to conda package specs with platform-specific handling.

use std::collections::HashMap;

use miette::Diagnostic;
use rattler_build_recipe::stage0::{ConditionalList, Item, SerializableMatchSpec, Value};
use rattler_conda_types::Platform;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::PackageMappingSource;
use crate::distro::Distro;
use crate::package_xml::{Dependency, PackageXml};

/// Errors that can occur during package mapping resolution.
#[derive(Debug, Error, Diagnostic)]
pub enum PackageMapError {
    #[error("unsupported platform: {platform}")]
    UnsupportedPlatform { platform: String },

    #[error(
        "unknown package map entry for '{dep_name}': expected 'ros', 'conda', or 'robostack' key"
    )]
    #[diagnostic(help("Check your robostack.yaml or extra-package-mappings configuration."))]
    UnknownMapEntry { dep_name: String },

    #[error(
        "version specifier can only be used for a package without constraint already present, \
         but found '{existing_spec}' for '{dep_name}' in the package map"
    )]
    VersionSpecConflict {
        dep_name: String,
        existing_spec: String,
    },

    #[error(
        "version specifier can only be used for one package, \
         but found {count} packages for '{dep_name}' in the package map"
    )]
    VersionSpecMultiplePackages { dep_name: String, count: usize },

    #[error(
        "incorrect version specification in package.xml for '{dep_name}': version is empty string"
    )]
    EmptyVersion { dep_name: String },

    #[error(
        "incorrect version specification in package.xml for '{dep_name}' at version '{version}'"
    )]
    InvalidVersion { dep_name: String, version: String },

    #[error("dependency '{dep_name}' cannot be specified by both `{op1}` and `{op2}`")]
    ConflictingConstraints {
        dep_name: String,
        op1: String,
        op2: String,
    },

    #[error("dependency '{dep_name}' cannot be specified by both `=` and an inequality")]
    EqualityWithInequality { dep_name: String },

    #[error("cannot merge version specifiers: '{spec1}' or '{spec2}' contains spaces")]
    MergeSpecsWithSpaces { spec1: String, spec2: String },
}

/// A single entry in a package mapping file.
///
/// Handles both flat lists and platform-specific dictionaries via serde
/// untagged.
pub type PackageMapEntry = HashMap<String, PlatformMapping>;

/// A mapping value that can be a flat list or a platform-specific dictionary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlatformMapping {
    /// A simple list of package names: `["pkg-a", "pkg-b"]`
    List(Vec<String>),
    /// A single string: `"pkg-a"`
    SingleString(String),
    /// Platform-specific mapping: `{"linux": ["pkg"], "osx": []}`
    PlatformSpecific(HashMap<String, Vec<String>>),
}

impl PlatformMapping {
    /// Resolve the mapping for a given platform string.
    fn resolve(&self, target_platform: &str) -> Vec<String> {
        match self {
            PlatformMapping::List(list) => list.clone(),
            PlatformMapping::SingleString(s) => vec![s.clone()],
            PlatformMapping::PlatformSpecific(map) => {
                map.get(target_platform).cloned().unwrap_or_default()
            }
        }
    }
}

/// Load and merge package map data from multiple sources.
/// Later sources (lower index) take priority — we iterate in reverse so
/// the first source in the list wins.
pub fn load_package_map_data(sources: &[PackageMappingSource]) -> HashMap<String, PackageMapEntry> {
    let mut result = HashMap::new();
    for source in sources.iter().rev() {
        match source {
            PackageMappingSource::File { path } => {
                if let Ok(content) = fs_err::read_to_string(path)
                    && let Ok(data) =
                        serde_yaml::from_str::<HashMap<String, PackageMapEntry>>(&content)
                {
                    result.extend(data);
                }
            }
            PackageMappingSource::Mapping(mapping) => {
                result.extend(mapping.clone());
            }
        }
    }
    result
}

/// Convert a ROS dependency to conda package spec(s).
pub fn rosdep_to_conda_package_spec(
    dep: &Dependency,
    distro: &Distro,
    host_platform: Platform,
    package_map_data: &HashMap<String, PackageMapEntry>,
) -> Result<Vec<String>, PackageMapError> {
    let target_platform = if host_platform.is_linux() {
        "linux"
    } else if host_platform.is_windows() {
        "win64"
    } else if host_platform.is_osx() {
        "osx"
    } else {
        return Err(PackageMapError::UnsupportedPlatform {
            platform: host_platform.to_string(),
        });
    };

    let spec_str = rosdep_nameless_matchspec(dep)?;

    // Not in package map -> assume it's a ROS package
    let Some(entry) = package_map_data.get(&dep.name) else {
        let ros_name = format!(
            "ros-{}-{}{}",
            distro.name,
            dep.name.replace('_', "-"),
            spec_str.as_deref().unwrap_or("")
        );
        return Ok(vec![ros_name]);
    };

    // Case 1: custom ROS dependency
    if let Some(ros_mapping) = entry.get("ros") {
        let packages = ros_mapping.resolve(target_platform);
        return Ok(packages
            .iter()
            .map(|name| {
                format!(
                    "ros-{}-{}{}",
                    distro.name,
                    name.replace('_', "-"),
                    spec_str.as_deref().unwrap_or("")
                )
            })
            .collect());
    }

    // Case 2: conda/robostack mapping
    let key = if entry.contains_key("robostack") {
        "robostack"
    } else if entry.contains_key("conda") {
        "conda"
    } else {
        return Err(PackageMapError::UnknownMapEntry {
            dep_name: dep.name.clone(),
        });
    };

    let mapping = entry.get(key).unwrap();
    let mut conda_packages = mapping.resolve(target_platform);
    let mut additional_packages = Vec::new();

    // Handle REQUIRE_GL
    if let Some(pos) = conda_packages.iter().position(|p| p == "REQUIRE_GL") {
        conda_packages.remove(pos);
        if target_platform.contains("linux") {
            additional_packages.push("libgl-devel".to_string());
        }
    }

    // Handle REQUIRE_OPENGL
    if let Some(pos) = conda_packages.iter().position(|p| p == "REQUIRE_OPENGL") {
        conda_packages.remove(pos);
        if target_platform.contains("linux") {
            additional_packages.extend(["libgl-devel".to_string(), "libopengl-devel".to_string()]);
        }
        if matches!(target_platform, "linux" | "osx" | "unix") {
            additional_packages.extend(["xorg-libx11".to_string(), "xorg-libxext".to_string()]);
        }
    }

    // Add version specifier if applicable
    if let Some(spec) = &spec_str {
        if conda_packages.len() == 1 {
            if conda_packages[0].contains(' ') {
                return Err(PackageMapError::VersionSpecConflict {
                    dep_name: dep.name.clone(),
                    existing_spec: conda_packages[0].clone(),
                });
            }
            conda_packages[0] = format!("{}{}", conda_packages[0], spec);
        } else if !conda_packages.is_empty() {
            return Err(PackageMapError::VersionSpecMultiplePackages {
                dep_name: dep.name.clone(),
                count: conda_packages.len(),
            });
        }
    }

    conda_packages.extend(additional_packages);
    Ok(conda_packages)
}

/// Format version constraints from a package.xml dependency to a nameless
/// matchspec string.
///
/// Returns `Some(" >=1.0,<2.0")` or `Some("==1.0")` or `None` if no
/// constraints.
pub fn rosdep_nameless_matchspec(dep: &Dependency) -> Result<Option<String>, PackageMapError> {
    let right_ineq = [&dep.version_lt, &dep.version_lte];
    let left_ineq = [&dep.version_gt, &dep.version_gte];
    let eq = &dep.version_eq;

    // Validate versions are non-empty and parseable if present
    for v in right_ineq
        .iter()
        .chain(left_ineq.iter())
        .chain(std::iter::once(&eq))
        .copied()
        .flatten()
    {
        if v.is_empty() {
            return Err(PackageMapError::EmptyVersion {
                dep_name: dep.name.clone(),
            });
        }
        if v.parse::<rattler_conda_types::Version>().is_err() {
            return Err(PackageMapError::InvalidVersion {
                dep_name: dep.name.clone(),
                version: v.clone(),
            });
        }
    }

    // Validate no conflicting constraints
    if right_ineq[0].is_some() && right_ineq[1].is_some() {
        return Err(PackageMapError::ConflictingConstraints {
            dep_name: dep.name.clone(),
            op1: "<".to_string(),
            op2: "<=".to_string(),
        });
    }
    if left_ineq[0].is_some() && left_ineq[1].is_some() {
        return Err(PackageMapError::ConflictingConstraints {
            dep_name: dep.name.clone(),
            op1: ">".to_string(),
            op2: ">=".to_string(),
        });
    }

    let some_inequality = right_ineq
        .iter()
        .chain(left_ineq.iter())
        .any(|v| v.is_some());
    if eq.is_some() && some_inequality {
        return Err(PackageMapError::EqualityWithInequality {
            dep_name: dep.name.clone(),
        });
    }

    if let Some(eq_val) = eq {
        return Ok(Some(format!("=={eq_val}")));
    }

    let mut parts = Vec::new();
    if let Some(v) = &dep.version_gt {
        parts.push(format!(">{v}"));
    }
    if let Some(v) = &dep.version_gte {
        parts.push(format!(">={v}"));
    }
    if let Some(v) = &dep.version_lt {
        parts.push(format!("<{v}"));
    }
    if let Some(v) = &dep.version_lte {
        parts.push(format!("<={v}"));
    }

    if parts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(format!(" {}", parts.join(","))))
    }
}

/// Convert a parsed PackageXml to conda requirements (build, host, run).
pub fn package_xml_to_conda_requirements(
    pkg: &PackageXml,
    distro: &Distro,
    host_platform: Platform,
    package_map_data: &HashMap<String, PackageMapEntry>,
) -> Result<ConditionalRequirements, PackageMapError> {
    // Build deps
    let mut build_deps: Vec<&Dependency> = Vec::new();
    build_deps.extend(
        pkg.dependencies
            .buildtool_depends
            .iter()
            .filter(|d| d.is_active()),
    );
    build_deps.extend(
        pkg.dependencies
            .buildtool_export_depends
            .iter()
            .filter(|d| d.is_active()),
    );
    build_deps.extend(
        pkg.dependencies
            .build_depends
            .iter()
            .filter(|d| d.is_active()),
    );
    build_deps.extend(
        pkg.dependencies
            .build_export_depends
            .iter()
            .filter(|d| d.is_active()),
    );
    build_deps.extend(
        pkg.dependencies
            .test_depends
            .iter()
            .filter(|d| d.is_active()),
    );
    build_deps.extend(pkg.dependencies.depend.iter().filter(|d| d.is_active()));

    // Add ros_workspace for ROS2
    let ros_workspace_dep;
    if !distro.is_ros1 {
        ros_workspace_dep = Dependency {
            name: "ros_workspace".to_string(),
            version_lt: None,
            version_lte: None,
            version_eq: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };
        build_deps.push(&ros_workspace_dep);
    }

    let mut conda_build_deps = Vec::new();
    for dep in &build_deps {
        conda_build_deps.extend(rosdep_to_conda_package_spec(
            dep,
            distro,
            host_platform,
            package_map_data,
        )?);
    }

    // Run deps
    let mut run_deps: Vec<&Dependency> = Vec::new();
    run_deps.extend(
        pkg.dependencies
            .run_depends
            .iter()
            .filter(|d| d.is_active()),
    );
    run_deps.extend(
        pkg.dependencies
            .exec_depends
            .iter()
            .filter(|d| d.is_active()),
    );
    run_deps.extend(
        pkg.dependencies
            .build_export_depends
            .iter()
            .filter(|d| d.is_active()),
    );
    run_deps.extend(
        pkg.dependencies
            .buildtool_export_depends
            .iter()
            .filter(|d| d.is_active()),
    );
    run_deps.extend(pkg.dependencies.depend.iter().filter(|d| d.is_active()));

    let mut conda_run_deps = Vec::new();
    for dep in &run_deps {
        conda_run_deps.extend(rosdep_to_conda_package_spec(
            dep,
            distro,
            host_platform,
            package_map_data,
        )?);
    }

    let build_items: Vec<Item<SerializableMatchSpec>> = conda_build_deps
        .iter()
        .map(|name| {
            Item::Value(Value::new_concrete(
                SerializableMatchSpec::from(name.as_str()),
                None,
            ))
        })
        .collect();

    let run_items: Vec<Item<SerializableMatchSpec>> = conda_run_deps
        .iter()
        .map(|name| {
            Item::Value(Value::new_concrete(
                SerializableMatchSpec::from(name.as_str()),
                None,
            ))
        })
        .collect();

    Ok(ConditionalRequirements {
        build: ConditionalList::new(build_items.clone()),
        host: ConditionalList::new(build_items),
        run: ConditionalList::new(run_items),
    })
}

/// Conda requirements split into build/host/run categories.
pub struct ConditionalRequirements {
    pub build: ConditionalList<SerializableMatchSpec>,
    pub host: ConditionalList<SerializableMatchSpec>,
    pub run: ConditionalList<SerializableMatchSpec>,
}

/// Extract a package name from an Item, if it's a concrete spec.
fn item_package_name(item: &Item<SerializableMatchSpec>) -> Option<String> {
    match item {
        Item::Value(v) => v
            .as_concrete()
            .and_then(|spec| spec.0.name.as_exact())
            .map(|name| name.as_normalized().to_string()),
        _ => None,
    }
}

/// Extract the template string from an Item, if it's a template.
fn item_template(item: &Item<SerializableMatchSpec>) -> Option<String> {
    match item {
        Item::Value(v) => v.as_template().map(|t| t.to_string()),
        _ => None,
    }
}

/// Extract the full spec string from a concrete Item.
fn item_spec_string(item: &Item<SerializableMatchSpec>) -> Option<String> {
    match item {
        Item::Value(v) => v.as_concrete().map(|s| s.to_string()),
        _ => None,
    }
}

/// Check if an item is a source dependency.
fn item_is_source(item: &Item<SerializableMatchSpec>) -> bool {
    match item {
        Item::Value(v) => v.as_concrete().is_some_and(|spec| spec.0.url.is_some()),
        _ => false,
    }
}

/// Normalize a spec by removing the package name prefix.
fn normalize_spec(spec: &str, package_name: &str) -> String {
    spec.strip_prefix(package_name)
        .unwrap_or(spec)
        .trim()
        .to_string()
}

/// Merge two version spec strings for the same package.
fn merge_specs(
    spec1: Option<&str>,
    spec2: Option<&str>,
    package_name: &str,
) -> Result<String, PackageMapError> {
    let v1 = spec1
        .map(|s| normalize_spec(s, package_name))
        .unwrap_or_default();
    let v2 = spec2
        .map(|s| normalize_spec(s, package_name))
        .unwrap_or_default();

    if v1.contains(' ') || v2.contains(' ') {
        return Err(PackageMapError::MergeSpecsWithSpaces {
            spec1: v1,
            spec2: v2,
        });
    }

    // Early out with *, empty, or ==
    if v1 == "*" || v1.is_empty() || v2.contains("==") || v1 == v2 {
        return Ok(spec2.unwrap_or("").to_string());
    }
    if v2 == "*" || v2.is_empty() || v1.contains("==") {
        return Ok(spec1.unwrap_or("").to_string());
    }

    Ok(format!("{package_name} {},{}", v1, v2))
}

/// Merge two ConditionalLists, deduplicating by package name and merging
/// version specs.
pub fn merge_conditional_lists(
    model: &ConditionalList<SerializableMatchSpec>,
    package: &ConditionalList<SerializableMatchSpec>,
) -> Result<ConditionalList<SerializableMatchSpec>, PackageMapError> {
    let items = merge_unique_items_vec(
        &model.iter().cloned().collect::<Vec<_>>(),
        &package.iter().cloned().collect::<Vec<_>>(),
    )?;
    Ok(ConditionalList::new(items))
}

/// Merge two lists of Items, deduplicating by package name and merging version
/// specs.
fn merge_unique_items_vec(
    model: &[Item<SerializableMatchSpec>],
    package: &[Item<SerializableMatchSpec>],
) -> Result<Vec<Item<SerializableMatchSpec>>, PackageMapError> {
    let mut result: Vec<Item<SerializableMatchSpec>> = Vec::new();
    let templates_in_model: Vec<String> = model.iter().filter_map(item_template).collect();

    for item in model.iter().chain(package.iter()) {
        if let Some(pkg_name) = item_package_name(item) {
            // Find existing item with same package name
            let existing_idx = result
                .iter()
                .position(|r| item_package_name(r).as_deref() == Some(pkg_name.as_str()));

            if let Some(idx) = existing_idx {
                let existing = &result[idx];
                if item_is_source(existing) {
                    // Keep existing source dep
                    continue;
                } else if item_is_source(item) {
                    // Replace with source dep
                    result[idx] = item.clone();
                } else {
                    // Merge specs
                    let existing_spec = item_spec_string(existing);
                    let new_spec = item_spec_string(item);
                    let merged =
                        merge_specs(existing_spec.as_deref(), new_spec.as_deref(), &pkg_name)?;
                    if !merged.is_empty() {
                        result[idx] = Item::Value(Value::new_concrete(
                            SerializableMatchSpec::from(merged.as_str()),
                            None,
                        ));
                    }
                }
            } else {
                result.push(item.clone());
            }
        } else if let Some(tmpl) = item_template(item) {
            if !templates_in_model.contains(&tmpl)
                || !result
                    .iter()
                    .any(|r| item_template(r).as_deref() == Some(tmpl.as_str()))
            {
                result.push(item.clone());
            }
        } else {
            result.push(item.clone());
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PackageMappingSource;
    use std::path::PathBuf;

    fn robostack_data() -> HashMap<String, PackageMapEntry> {
        let content = include_str!("../robostack.yaml");
        serde_yaml::from_str(content).unwrap()
    }

    fn jazzy_distro() -> Distro {
        Distro::builder("jazzy").build()
    }

    #[test]
    fn test_package_loading() {
        let test_data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data");
        let other_map = test_data.join("other_package_map.yaml");
        let robostack_file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("robostack.yaml");

        let result = load_package_map_data(&[
            PackageMappingSource::File { path: other_map },
            PackageMappingSource::File {
                path: robostack_file,
            },
        ]);

        assert!(result.contains_key("new_package"), "Should be added");
        // alsa-oss should be overwritten by other_package_map
        let alsa = result.get("alsa-oss").unwrap();
        assert!(alsa.contains_key("conda"), "Should have conda key");
        assert!(
            !alsa.contains_key("robostack"),
            "Should be overwritten due to priority"
        );
        assert!(result.contains_key("zlib"), "Should still be present");
    }

    #[test]
    fn test_robostack_target_platform_linux() {
        let distro = jazzy_distro();
        let package_map = robostack_data();

        let dep = Dependency {
            name: "acl".to_string(),
            version_lt: None,
            version_lte: None,
            version_eq: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Linux64, &package_map).unwrap();
        assert_eq!(result, vec!["libacl"]);
    }

    #[test]
    fn test_robostack_target_platform_osx() {
        let distro = jazzy_distro();
        let package_map = robostack_data();

        let dep = Dependency {
            name: "acl".to_string(),
            version_lt: None,
            version_lte: None,
            version_eq: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Osx64, &package_map).unwrap();
        assert_eq!(result, Vec::<String>::new());
    }

    #[test]
    fn test_robostack_target_platform_windows() {
        let distro = jazzy_distro();
        let package_map = robostack_data();

        let dep = Dependency {
            name: "binutils".to_string(),
            version_lt: None,
            version_lte: None,
            version_eq: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Win64, &package_map).unwrap();
        assert_eq!(result, Vec::<String>::new());
    }

    #[test]
    fn test_robostack_target_platform_cross_platform() {
        let distro = jazzy_distro();
        let package_map = robostack_data();

        // libudev-dev
        let dep = Dependency {
            name: "libudev-dev".to_string(),
            version_lt: None,
            version_lte: None,
            version_eq: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };

        let linux =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Linux64, &package_map).unwrap();
        let osx =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Osx64, &package_map).unwrap();
        let win =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Win64, &package_map).unwrap();

        assert_eq!(linux, vec!["libusb", "libudev"]);
        assert_eq!(osx, vec!["libusb"]);
        assert_eq!(win, vec!["libusb"]);

        // libomp-dev
        let dep = Dependency {
            name: "libomp-dev".to_string(),
            version_lt: None,
            version_lte: None,
            version_eq: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };

        let linux =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Linux64, &package_map).unwrap();
        let osx =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Osx64, &package_map).unwrap();
        let win =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Win64, &package_map).unwrap();

        assert_eq!(linux, vec!["libgomp"]);
        assert_eq!(osx, vec!["llvm-openmp"]);
        assert_eq!(win, Vec::<String>::new());
    }

    #[test]
    fn test_robostack_require_opengl_handling() {
        let distro = jazzy_distro();
        let package_map = robostack_data();

        let dep = Dependency {
            name: "opengl".to_string(),
            version_lt: None,
            version_lte: None,
            version_eq: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };

        let linux =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Linux64, &package_map).unwrap();
        let osx =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Osx64, &package_map).unwrap();
        let win =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Win64, &package_map).unwrap();

        assert!(linux.contains(&"libgl-devel".to_string()));
        assert!(linux.contains(&"libopengl-devel".to_string()));
        assert!(linux.contains(&"xorg-libx11".to_string()));
        assert!(linux.contains(&"xorg-libxext".to_string()));

        assert!(osx.contains(&"xorg-libx11".to_string()));
        assert!(osx.contains(&"xorg-libxext".to_string()));
        assert!(!osx.contains(&"libgl-devel".to_string()));

        assert_eq!(win, Vec::<String>::new());
    }

    #[test]
    fn test_rosdep_to_conda_for_unknown_package() {
        let distro = jazzy_distro();

        let dep = Dependency {
            name: "ament_cmake".to_string(),
            version_eq: Some("1.2.3".to_string()),
            version_lt: None,
            version_lte: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Linux64, &HashMap::new())
                .unwrap();
        assert_eq!(result, vec!["ros-jazzy-ament-cmake==1.2.3"]);
    }

    #[test]
    fn test_rosdep_to_conda_range_version() {
        let distro = jazzy_distro();

        let dep = Dependency {
            name: "rclcpp".to_string(),
            version_gte: Some("18.0.0".to_string()),
            version_lt: Some("20.0.0".to_string()),
            version_eq: None,
            version_lte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Linux64, &HashMap::new())
                .unwrap();
        assert_eq!(result, vec!["ros-jazzy-rclcpp >=18.0.0,<20.0.0"]);
    }

    #[test]
    fn test_rosdep_to_conda_ros_map_entries() {
        let distro = jazzy_distro();
        let mut package_map: HashMap<String, PackageMapEntry> = HashMap::new();
        let mut entry = PackageMapEntry::new();
        entry.insert(
            "ros".to_string(),
            PlatformMapping::List(vec!["foo_util".to_string()]),
        );
        package_map.insert("custom_ros_dep".to_string(), entry);

        let dep = Dependency {
            name: "custom_ros_dep".to_string(),
            version_gte: Some("3.1".to_string()),
            version_lt: None,
            version_lte: None,
            version_eq: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Linux64, &package_map).unwrap();
        assert_eq!(result, vec!["ros-jazzy-foo-util >=3.1"]);
    }

    #[test]
    fn test_spec_with_entry_in_map_error() {
        let distro = jazzy_distro();
        let package_map = robostack_data();

        let dep = Dependency {
            name: "xtensor".to_string(),
            version_eq: Some("2.0".to_string()),
            version_lt: None,
            version_lte: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };

        let result = rosdep_to_conda_package_spec(&dep, &distro, Platform::current(), &package_map);
        assert!(matches!(
            result,
            Err(PackageMapError::VersionSpecConflict { .. })
        ));
    }

    #[test]
    fn test_spec_with_multiple_entries_error() {
        let distro = jazzy_distro();
        let package_map = robostack_data();

        let dep = Dependency {
            name: "boost".to_string(),
            version_eq: Some("2.0".to_string()),
            version_lt: None,
            version_lte: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };

        let result = rosdep_to_conda_package_spec(&dep, &distro, Platform::current(), &package_map);
        assert!(matches!(
            result,
            Err(PackageMapError::VersionSpecMultiplePackages { .. })
        ));
    }

    #[test]
    fn test_conflicting_version_specs() {
        let dep = Dependency {
            name: "rclcpp".to_string(),
            version_gt: Some("18.0.0".to_string()),
            version_gte: Some("20.0.0".to_string()),
            version_lt: None,
            version_lte: None,
            version_eq: None,
            condition: None,
            evaluated_condition: None,
        };

        let result = rosdep_nameless_matchspec(&dep);
        assert!(matches!(
            result,
            Err(PackageMapError::ConflictingConstraints { .. })
        ));
    }

    #[test]
    fn test_version_constraint_eq() {
        let dep = Dependency {
            name: "test".to_string(),
            version_eq: Some("1.0".to_string()),
            version_lt: None,
            version_lte: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };
        assert_eq!(
            rosdep_nameless_matchspec(&dep).unwrap(),
            Some("==1.0".to_string())
        );
    }

    #[test]
    fn test_version_constraint_range() {
        let dep = Dependency {
            name: "test".to_string(),
            version_gte: Some("1.0".to_string()),
            version_lt: Some("2.0".to_string()),
            version_eq: None,
            version_lte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };
        assert_eq!(
            rosdep_nameless_matchspec(&dep).unwrap(),
            Some(" >=1.0,<2.0".to_string())
        );
    }

    #[test]
    fn test_version_constraint_none() {
        let dep = Dependency {
            name: "test".to_string(),
            version_eq: None,
            version_lt: None,
            version_lte: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        };
        assert_eq!(rosdep_nameless_matchspec(&dep).unwrap(), None);
    }

    #[test]
    fn test_merge_unique_items_basic() {
        let list1 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic"),
            None,
        ))];
        let list2 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic <=2.0"),
            None,
        ))];

        let result = merge_unique_items_vec(&list1, &list2).unwrap();
        assert_eq!(result.len(), 1);

        let spec = item_spec_string(&result[0]).unwrap();
        assert_eq!(spec, "ros-noetic <=2.0");
    }

    #[test]
    fn test_merge_unique_items_different_packages() {
        let list1 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic >=2.0"),
            None,
        ))];
        let list2 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic2 <=2.0,<3.0"),
            None,
        ))];

        let result = merge_unique_items_vec(&list1, &list2).unwrap();
        assert_eq!(result.len(), 2);
    }

    // Ported from test_spec_merging.py

    #[test]
    fn test_merge_with_star_items() {
        let list1 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic *"),
            None,
        ))];
        let list2 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic <=2.0"),
            None,
        ))];

        // Both orderings should give the same result
        for result in [
            merge_unique_items_vec(&list1, &list2).unwrap(),
            merge_unique_items_vec(&list2, &list1).unwrap(),
        ] {
            assert_eq!(result.len(), 1);
            assert_eq!(item_spec_string(&result[0]).unwrap(), "ros-noetic <=2.0");
        }
    }

    #[test]
    fn test_merge_equal_items() {
        let list1 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic ==2.0"),
            None,
        ))];
        let list2 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic <=2.0"),
            None,
        ))];

        for result in [
            merge_unique_items_vec(&list1, &list2).unwrap(),
            merge_unique_items_vec(&list2, &list1).unwrap(),
        ] {
            assert_eq!(result.len(), 1);
            assert_eq!(item_spec_string(&result[0]).unwrap(), "ros-noetic ==2.0");
        }
    }

    #[test]
    fn test_merge_multiple_specs_items() {
        let list1 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic >=2.0"),
            None,
        ))];
        let list2 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic <=2.0,<3.0"),
            None,
        ))];

        let result = merge_unique_items_vec(&list1, &list2).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(
            item_spec_string(&result[0]).unwrap(),
            "ros-noetic >=2.0,<=2.0,<3.0"
        );
    }

    #[test]
    fn test_merge_specs_with_spaces() {
        let list1 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic 2.* noetic"),
            None,
        ))];
        let list2 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic <=2.0,<3.0"),
            None,
        ))];

        let result = merge_unique_items_vec(&list1, &list2);
        assert!(matches!(
            result,
            Err(PackageMapError::MergeSpecsWithSpaces { .. })
        ));
    }

    #[test]
    fn test_merge_specs_with_none() {
        let list1 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic"),
            None,
        ))];
        let list2 = vec![Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("ros-noetic <=2.0,<3.0"),
            None,
        ))];

        let result = merge_unique_items_vec(&list1, &list2).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(
            item_spec_string(&result[0]).unwrap(),
            "ros-noetic <=2.0,<3.0"
        );
    }

    #[test]
    fn test_rosdep_to_conda_for_unknown_dep_with_gt_version() {
        let distro = jazzy_distro();

        let dep = Dependency {
            name: "customlib".to_string(),
            version_gt: Some("1.0.0".to_string()),
            version_lt: None,
            version_lte: None,
            version_eq: None,
            version_gte: None,
            condition: None,
            evaluated_condition: None,
        };

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Platform::Linux64, &HashMap::new())
                .unwrap();
        assert_eq!(result, vec!["ros-jazzy-customlib >1.0.0"]);
    }
}
