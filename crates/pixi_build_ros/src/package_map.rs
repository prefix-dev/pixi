//! Package mapping resolution between ROS and conda package names.
//!
//! Loads the bundled RoboStack mappings and user-provided ones, and resolves ROS
//! dependency names to conda package specs with platform-specific handling.
//!
//! The bundled `robostack.yaml` (every distribution) and `robostack_distros.yaml`
//! (per distribution, over it) are generated from the RoboStack repository, which
//! RoboStack's own recipe generator reads too: both resolve rosdep keys the same way.

use std::cell::LazyCell;
use std::collections::HashMap;

use miette::Diagnostic;
use rattler_build_jinja::{Jinja, JinjaConfig};
use rattler_build_recipe::stage0::{ConditionalList, Item, SerializableMatchSpec, Value};
use rattler_conda_types::{MatchSpec, ParseStrictness, Subdir};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::PackageMappingSource;
use crate::distro::{Distro, RosVersion};
use crate::package_xml::{Dependency, PackageXml};

/// Errors that can occur during package mapping resolution.
#[derive(Debug, Error, Diagnostic)]
pub enum PackageMapError {
    #[error("failed to read package map {path}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid package map {name}")]
    ParseFile {
        name: String,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("failed to render package mapping '{spec}' for '{dep_name}'")]
    RenderSpec {
        dep_name: String,
        spec: String,
        #[source]
        source: minijinja::Error,
    },

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

    #[error("dependency '{dep_name}' cannot have both build strings `{build1}` and `{build2}`")]
    ConflictingBuildStrings {
        dep_name: String,
        build1: String,
        build2: String,
    },

    #[error("cannot merge the specifier '{spec}': it has more than a version and a build string")]
    MergeSpecWithSpaces { spec: String },

    #[error("rosdep '{dep_name}' resolved to an invalid conda package spec '{spec}'")]
    #[diagnostic(help("Map '{dep_name}' to a valid conda package via `extra-package-mappings`."))]
    InvalidCondaPackageSpec {
        dep_name: String,
        spec: String,
        #[source]
        source: rattler_conda_types::ParseMatchSpecError,
    },
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
    /// Platform-specific mapping, by subdir (`linux-aarch64`) or by OS (`linux`,
    /// `osx`, `win64`, `emscripten`): `{"linux": ["pkg"], "osx": []}` (`osx:`
    /// without a value is the same as `[]`)
    PlatformSpecific(HashMap<String, Option<Vec<String>>>),
}

impl PlatformMapping {
    /// The packages for a platform: its subdir's entry, else its OS's, else none.
    fn resolve(&self, platform: Subdir) -> Vec<String> {
        match self {
            PlatformMapping::List(list) => list.clone(),
            PlatformMapping::SingleString(s) => vec![s.clone()],
            PlatformMapping::PlatformSpecific(map) => map
                .get(platform.as_str())
                .or_else(|| {
                    let os = match platform.only_platform()? {
                        "win" => "win64",
                        os => os,
                    };
                    map.get(os)
                })
                .cloned()
                .flatten()
                .unwrap_or_default(),
        }
    }
}

/// What a rosdep key maps to on a platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappedPackages {
    /// ROS packages (`ros:`), by their ROS name
    Ros(Vec<String>),
    /// conda package specs (`robostack:` or `conda:`)
    Conda(Vec<String>),
}

/// What `name` maps to on `platform`, or `None` when no mapping has it.
pub fn lookup(
    package_map_data: &HashMap<String, PackageMapEntry>,
    name: &str,
    platform: Subdir,
) -> Result<Option<MappedPackages>, PackageMapError> {
    let Some(entry) = package_map_data.get(name) else {
        return Ok(None);
    };
    if let Some(ros) = entry.get("ros") {
        return Ok(Some(MappedPackages::Ros(ros.resolve(platform))));
    }
    match entry.get("robostack").or_else(|| entry.get("conda")) {
        Some(conda) => Ok(Some(MappedPackages::Conda(conda.resolve(platform)))),
        None => Err(PackageMapError::UnknownMapEntry {
            dep_name: name.to_string(),
        }),
    }
}

fn parse_map(
    name: &str,
    content: &str,
) -> Result<HashMap<String, PackageMapEntry>, PackageMapError> {
    serde_yaml::from_str::<Option<_>>(content)
        .map(Option::unwrap_or_default)
        .map_err(|source| PackageMapError::ParseFile {
            name: name.to_string(),
            source,
        })
}

/// The bundled RoboStack mappings for a distribution: its own entries over the
/// ones for every distribution.
pub fn bundled_package_map(
    distro: &str,
) -> Result<HashMap<String, PackageMapEntry>, PackageMapError> {
    let mut map = parse_map("robostack.yaml", include_str!("../robostack.yaml"))?;
    let mut distros: HashMap<String, Option<HashMap<String, PackageMapEntry>>> =
        serde_yaml::from_str(include_str!("../robostack_distros.yaml")).map_err(|source| {
            PackageMapError::ParseFile {
                name: "robostack_distros.yaml".to_string(),
                source,
            }
        })?;
    map.extend(distros.remove(distro).flatten().unwrap_or_default());
    Ok(map)
}

/// Load and merge package map data from multiple sources: the first source that
/// has a key wins.
pub fn load_package_map_data(
    sources: &[PackageMappingSource],
) -> Result<HashMap<String, PackageMapEntry>, PackageMapError> {
    let mut result = HashMap::new();
    for source in sources.iter().rev() {
        match source {
            PackageMappingSource::File { path } => {
                let content =
                    fs_err::read_to_string(path).map_err(|source| PackageMapError::ReadFile {
                        path: path.display().to_string(),
                        source,
                    })?;
                result.extend(parse_map(&path.display().to_string(), &content)?);
            }
            PackageMappingSource::Mapping(mapping) => {
                result.extend(mapping.clone());
            }
        }
    }
    Ok(result)
}

/// Convert a ROS dependency to conda package spec(s).
///
/// Each resolved spec is validated as a strict conda [`MatchSpec`]; an invalid
/// name (e.g. `crypto++`) errors here instead of panicking downstream.
pub fn rosdep_to_conda_package_spec(
    dep: &Dependency,
    distro: &Distro,
    host_platform: Subdir,
    package_map_data: &HashMap<String, PackageMapEntry>,
) -> Result<Vec<String>, PackageMapError> {
    let specs = resolve_rosdep_to_conda_package_spec(dep, distro, host_platform, package_map_data)?;

    for spec in &specs {
        if spec.trim().is_empty() {
            continue;
        }
        MatchSpec::from_str(spec, ParseStrictness::Strict).map_err(|source| {
            PackageMapError::InvalidCondaPackageSpec {
                dep_name: dep.name.clone(),
                spec: spec.clone(),
                source,
            }
        })?;
    }

    Ok(specs)
}

/// Resolve a ROS dependency to conda package spec(s) without validation.
fn resolve_rosdep_to_conda_package_spec(
    dep: &Dependency,
    distro: &Distro,
    host_platform: Subdir,
    package_map_data: &HashMap<String, PackageMapEntry>,
) -> Result<Vec<String>, PackageMapError> {
    let spec_str = rosdep_nameless_matchspec(dep)?;
    let ros_name = |name: &str| {
        format!(
            "{}{}",
            distro.conda_name(name),
            spec_str.as_deref().unwrap_or("")
        )
    };

    let mut conda_packages = match lookup(package_map_data, &dep.name, host_platform)? {
        // Not in package map -> assume it's a ROS package
        None => return Ok(vec![ros_name(&dep.name)]),
        Some(MappedPackages::Ros(names)) => {
            return Ok(names.iter().map(|n| ros_name(n)).collect());
        }
        Some(MappedPackages::Conda(packages)) => packages,
    };

    let jinja = LazyCell::new(|| {
        Jinja::new(JinjaConfig {
            target_platform: host_platform,
            host_platform,
            ..Default::default()
        })
    });
    for package in &mut conda_packages {
        if package.contains("${{") {
            *package = jinja
                .render_str(package)
                .map_err(|source| PackageMapError::RenderSpec {
                    dep_name: dep.name.clone(),
                    spec: package.clone(),
                    source,
                })?;
        }
    }

    // A version specifier applies to the mapping's first package; the others are
    // its companions (e.g. libgl-devel for Qt on Linux).
    if let (Some(spec), Some(first)) = (&spec_str, conda_packages.first_mut()) {
        if first.contains(' ') {
            return Err(PackageMapError::VersionSpecConflict {
                dep_name: dep.name.clone(),
                existing_spec: first.clone(),
            });
        }
        first.push_str(spec);
    }

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
    host_platform: Subdir,
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
    if distro.version == RosVersion::Ros2 {
        ros_workspace_dep = Dependency::from("ros_workspace");
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
pub(crate) fn item_package_name(item: &Item<SerializableMatchSpec>) -> Option<String> {
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

/// A spec without the package name, split into its version and build string.
fn version_and_build(
    spec: Option<&str>,
    package_name: &str,
) -> Result<(String, Option<String>), PackageMapError> {
    let spec = spec
        .map(|s| normalize_spec(s, package_name))
        .unwrap_or_default();
    let Some((version, build)) = spec.split_once(' ') else {
        return Ok((spec, None));
    };
    if build.contains(' ') {
        return Err(PackageMapError::MergeSpecWithSpaces { spec });
    }
    Ok((version.to_string(), Some(build.to_string())))
}

/// Merge two specs for the same package: their version constraints, and the
/// build string that at most one of them has (e.g. the distro mutex's pin to a
/// distribution).
fn merge_specs(
    spec1: Option<&str>,
    spec2: Option<&str>,
    package_name: &str,
) -> Result<String, PackageMapError> {
    let (v1, b1) = version_and_build(spec1, package_name)?;
    let (v2, b2) = version_and_build(spec2, package_name)?;

    let build = match (b1, b2) {
        (Some(b1), Some(b2)) if b1 != b2 => {
            return Err(PackageMapError::ConflictingBuildStrings {
                dep_name: package_name.to_string(),
                build1: b1,
                build2: b2,
            });
        }
        (b1, b2) => b1.or(b2),
    };

    let version = if v1 == "*" || v1.is_empty() || v2.contains("==") || v1 == v2 {
        v2
    } else if v2 == "*" || v2.is_empty() || v1.contains("==") {
        v1
    } else {
        format!("{v1},{v2}")
    };

    Ok(match (version.as_str(), build) {
        (_, Some(build)) if version.is_empty() => format!("{package_name} * {build}"),
        (_, Some(build)) => format!("{package_name} {version} {build}"),
        ("", None) if spec1.is_none() && spec2.is_none() => String::new(),
        ("", None) => package_name.to_string(),
        (_, None) => format!("{package_name} {version}"),
    })
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
        bundled_package_map("jazzy").unwrap()
    }

    fn jazzy_distro() -> Distro {
        Distro::new("jazzy")
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
        ])
        .unwrap();

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

        let dep = Dependency::from("acl");

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Subdir::Linux64, &package_map).unwrap();
        assert_eq!(result, vec!["libacl"]);
    }

    #[test]
    fn test_robostack_target_platform_osx() {
        let distro = jazzy_distro();
        let package_map = robostack_data();

        let dep = Dependency::from("acl");

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Subdir::Osx64, &package_map).unwrap();
        assert_eq!(result, Vec::<String>::new());
    }

    #[test]
    fn test_robostack_target_platform_windows() {
        let distro = jazzy_distro();
        let package_map = robostack_data();

        let dep = Dependency::from("binutils");

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Subdir::Win64, &package_map).unwrap();
        assert_eq!(result, Vec::<String>::new());
    }

    #[test]
    fn test_robostack_target_platform_cross_platform() {
        let distro = jazzy_distro();
        let package_map = robostack_data();

        // libudev-dev
        let dep = Dependency::from("libudev-dev");

        let linux =
            rosdep_to_conda_package_spec(&dep, &distro, Subdir::Linux64, &package_map).unwrap();
        let osx = rosdep_to_conda_package_spec(&dep, &distro, Subdir::Osx64, &package_map).unwrap();
        let win = rosdep_to_conda_package_spec(&dep, &distro, Subdir::Win64, &package_map).unwrap();

        assert_eq!(linux, vec!["libusb", "libudev"]);
        assert_eq!(osx, vec!["libusb"]);
        assert_eq!(win, vec!["libusb"]);

        // libomp-dev
        let dep = Dependency::from("libomp-dev");

        let linux =
            rosdep_to_conda_package_spec(&dep, &distro, Subdir::Linux64, &package_map).unwrap();
        let osx = rosdep_to_conda_package_spec(&dep, &distro, Subdir::Osx64, &package_map).unwrap();
        let win = rosdep_to_conda_package_spec(&dep, &distro, Subdir::Win64, &package_map).unwrap();

        assert_eq!(linux, vec!["libgomp"]);
        assert_eq!(osx, vec!["llvm-openmp"]);
        assert_eq!(win, Vec::<String>::new());
    }

    #[test]
    fn test_platform_keys_subdir_before_os() {
        let package_map: HashMap<String, PackageMapEntry> = serde_yaml::from_str(
            "gl: {robostack: {linux: [libgl-devel], linux-aarch64: [], win64: [opengl32], osx: }}",
        )
        .unwrap();
        let resolve = |platform| match lookup(&package_map, "gl", platform).unwrap() {
            Some(MappedPackages::Conda(packages)) => packages,
            other => panic!("{other:?}"),
        };
        assert_eq!(resolve(Subdir::Linux64), vec!["libgl-devel"]);
        assert_eq!(resolve(Subdir::LinuxAarch64), Vec::<String>::new());
        assert_eq!(resolve(Subdir::Win64), vec!["opengl32"]);
        assert_eq!(resolve(Subdir::Osx64), Vec::<String>::new());
        assert_eq!(
            lookup(&package_map, "other", Subdir::Linux64).unwrap(),
            None
        );
    }

    #[test]
    fn test_bundled_distro_mappings_override_shared_ones() {
        let humble = bundled_package_map("humble").unwrap();
        let kilted = bundled_package_map("kilted").unwrap();
        let rolling = bundled_package_map("rolling").unwrap();
        let qt = |map| lookup(map, "qtbase5-dev", Subdir::Osx64).unwrap();
        for qt5 in [&humble, &kilted] {
            assert_eq!(
                qt(qt5),
                Some(MappedPackages::Conda(vec!["qt-main".to_string()]))
            );
        }
        assert_eq!(
            lookup(&kilted, "python3-qt5-bindings", Subdir::Osx64).unwrap(),
            Some(MappedPackages::Conda(vec![
                "pyqt".to_string(),
                "pyqt-builder".to_string()
            ]))
        );
        assert_eq!(
            qt(&rolling),
            Some(MappedPackages::Conda(vec!["qt6-main".to_string()]))
        );
    }

    #[test]
    fn test_cuda_mapping_uses_the_target_platform() {
        let package_map = robostack_data();
        for platform in [Subdir::Linux64, Subdir::LinuxAarch64] {
            let specs = rosdep_to_conda_package_spec(
                &Dependency::from("nvidia-cuda"),
                &jazzy_distro(),
                platform,
                &package_map,
            )
            .unwrap();
            assert_eq!(
                specs,
                vec![
                    "cuda-cudart-dev".to_string(),
                    format!("cuda-nvcc-dev_{platform}"),
                ]
            );
        }
    }

    #[test]
    fn test_unreadable_package_map_is_an_error() {
        let missing = PackageMappingSource::File {
            path: PathBuf::from("does-not-exist.yaml"),
        };
        assert!(matches!(
            load_package_map_data(&[missing]),
            Err(PackageMapError::ReadFile { .. })
        ));
    }

    #[test]
    fn test_rosdep_to_conda_for_unknown_package() {
        let distro = jazzy_distro();

        let dep = Dependency {
            version_eq: Some("1.2.3".to_string()),
            ..Dependency::from("ament_cmake")
        };

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Subdir::Linux64, &HashMap::new()).unwrap();
        assert_eq!(result, vec!["ros2-ament-cmake==1.2.3"]);
    }

    #[test]
    fn test_rosdep_to_conda_range_version() {
        let distro = jazzy_distro();

        let dep = Dependency {
            version_gte: Some("18.0.0".to_string()),
            version_lt: Some("20.0.0".to_string()),
            ..Dependency::from("rclcpp")
        };

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Subdir::Linux64, &HashMap::new()).unwrap();
        assert_eq!(result, vec!["ros2-rclcpp >=18.0.0,<20.0.0"]);
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
            version_gte: Some("3.1".to_string()),
            ..Dependency::from("custom_ros_dep")
        };

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Subdir::Linux64, &package_map).unwrap();
        assert_eq!(result, vec!["ros2-foo-util >=3.1"]);
    }

    #[test]
    fn test_spec_with_entry_in_map_error() {
        let distro = jazzy_distro();
        let package_map: HashMap<String, PackageMapEntry> =
            serde_yaml::from_str("xtensor: {robostack: [xtensor ==0.24.7]}").unwrap();

        let dep = Dependency {
            version_eq: Some("2.0".to_string()),
            ..Dependency::from("xtensor")
        };

        let result = rosdep_to_conda_package_spec(
            &dep,
            &distro,
            Subdir::current().unwrap_or(Subdir::NoArch),
            &package_map,
        );
        assert!(matches!(
            result,
            Err(PackageMapError::VersionSpecConflict { .. })
        ));
    }

    #[test]
    fn test_spec_applies_to_the_first_package() {
        let distro = jazzy_distro();
        let package_map: HashMap<String, PackageMapEntry> =
            serde_yaml::from_str("qt: {robostack: {linux: [qt-main, libgl-devel]}}").unwrap();

        let dep = Dependency {
            version_gte: Some("5.15".to_string()),
            ..Dependency::from("qt")
        };

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Subdir::Linux64, &package_map).unwrap();
        assert_eq!(result, vec!["qt-main >=5.15", "libgl-devel"]);
    }

    #[test]
    fn test_conflicting_version_specs() {
        let dep = Dependency {
            version_gt: Some("18.0.0".to_string()),
            version_gte: Some("20.0.0".to_string()),
            ..Dependency::from("rclcpp")
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
            version_eq: Some("1.0".to_string()),
            ..Dependency::from("test")
        };
        assert_eq!(
            rosdep_nameless_matchspec(&dep).unwrap(),
            Some("==1.0".to_string())
        );
    }

    #[test]
    fn test_version_constraint_range() {
        let dep = Dependency {
            version_gte: Some("1.0".to_string()),
            version_lt: Some("2.0".to_string()),
            ..Dependency::from("test")
        };
        assert_eq!(
            rosdep_nameless_matchspec(&dep).unwrap(),
            Some(" >=1.0,<2.0".to_string())
        );
    }

    #[test]
    fn test_version_constraint_none() {
        let dep = Dependency::from("test");
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
    fn test_merge_specs_with_build_strings() {
        let spec = |s: &str| {
            vec![Item::Value(Value::new_concrete(
                SerializableMatchSpec::from(s),
                None,
            ))]
        };

        // a version and a build string, e.g. the distro mutex's pin
        let result = merge_unique_items_vec(
            &spec("ros2-distro-mutex 0.5.*"),
            &spec("ros2-distro-mutex * jazzy_*"),
        )
        .unwrap();
        assert_eq!(
            item_spec_string(&result[0]).unwrap(),
            "ros2-distro-mutex 0.5.* jazzy_*"
        );

        let result = merge_unique_items_vec(
            &spec("ros-noetic 2.* noetic"),
            &spec("ros-noetic <=2.0,<3.0"),
        )
        .unwrap();
        assert_eq!(
            item_spec_string(&result[0]).unwrap(),
            "ros-noetic 2.*,<=2.0,<3.0 noetic"
        );

        let result = merge_unique_items_vec(
            &spec("ros-noetic 2.* noetic"),
            &spec("ros-noetic * melodic"),
        );
        assert!(matches!(
            result,
            Err(PackageMapError::ConflictingBuildStrings { .. })
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
            version_gt: Some("1.0.0".to_string()),
            ..Dependency::from("customlib")
        };

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Subdir::Linux64, &HashMap::new()).unwrap();
        assert_eq!(result, vec!["ros2-customlib >1.0.0"]);
    }

    #[test]
    fn test_lttng_resolves_per_platform() {
        // Regression for prefix-dev/pixi#6288: `lttng-ust` on linux, nothing
        // elsewhere (was a leaked `${{` selector).
        let distro = jazzy_distro();
        let package_map = robostack_data();
        let dep = Dependency::from("liblttng-ust-dev");

        let linux =
            rosdep_to_conda_package_spec(&dep, &distro, Subdir::Linux64, &package_map).unwrap();
        assert_eq!(linux, vec!["lttng-ust"]);

        let osx = rosdep_to_conda_package_spec(&dep, &distro, Subdir::Osx64, &package_map).unwrap();
        assert_eq!(osx, Vec::<String>::new());

        let win = rosdep_to_conda_package_spec(&dep, &distro, Subdir::Win64, &package_map).unwrap();
        assert_eq!(win, Vec::<String>::new());
    }

    #[test]
    fn test_crypto_pp_maps_to_cryptopp() {
        let distro = jazzy_distro();
        let package_map = robostack_data();
        let dep = Dependency::from("crypto++");

        let result =
            rosdep_to_conda_package_spec(&dep, &distro, Subdir::Linux64, &package_map).unwrap();
        assert_eq!(result, vec!["cryptopp"]);
    }

    #[test]
    fn test_unmapped_invalid_conda_name_errors() {
        // Unmapped rosdep with an invalid conda name must error, not panic.
        let distro = jazzy_distro();
        let dep = Dependency::from("crypto++");

        let result = rosdep_to_conda_package_spec(&dep, &distro, Subdir::Linux64, &HashMap::new());
        assert!(matches!(
            result,
            Err(PackageMapError::InvalidCondaPackageSpec { .. })
        ));
    }
}
