//! Discover sibling ROS packages inside a workspace.
//!
//! Walks a workspace root looking for `package.xml` files, honoring the
//! standard colcon ignore markers (`COLCON_IGNORE`, `AMENT_IGNORE`,
//! `CATKIN_IGNORE`). Returns the discovered packages keyed by the `<name>`
//! element together with a glob list suitable for pixi's metadata
//! invalidation cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use fs_err as fs;
use miette::Diagnostic;
use pixi_build_types::InputGlobSet;
use pixi_glob::GlobSet;
use rattler_build_recipe::stage0::{ConditionalList, Item, SerializableMatchSpec, Value};
use thiserror::Error;
use url::Url;

use crate::package_map::item_package_name;
use crate::package_xml::{PackageXml, PackageXmlError};

const PACKAGE_XML: &str = "package.xml";
const IGNORE_MARKERS: &[&str] = &["COLCON_IGNORE", "AMENT_IGNORE", "CATKIN_IGNORE"];

/// Result of walking a workspace root for ROS packages.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceDiscovery {
    /// Map from the `<name>` of each discovered package to the absolute path of
    /// its `package.xml`.
    pub packages: HashMap<String, PathBuf>,
    /// Structured glob description of the discovery, suitable for pixi's
    /// metadata invalidation cache.  Pixi replays the same walk later
    /// using the marker semantics carried here; we deliberately don't
    /// emit a flat `Vec<String>` form because per-pruned-dir exclusions
    /// (`!build/**`, `!log/**`, ...) and the hidden-folder exclusion
    /// (`!**/.*/**`) can't be expressed losslessly without the markers
    /// the structured form carries.
    pub input_glob_set: InputGlobSet,
}

#[derive(Debug, Error, Diagnostic)]
pub enum WorkspaceDiscoveryError {
    #[error("duplicate ROS package name `{name}` declared by both:\n  - {first}\n  - {second}")]
    #[diagnostic(help(
        "Each ROS package in a workspace must have a unique <name> in its package.xml."
    ))]
    DuplicateName {
        name: String,
        first: PathBuf,
        second: PathBuf,
    },

    #[error("workspace walk failed at {path}")]
    Walk {
        path: PathBuf,
        #[source]
        source: Box<pixi_glob::GlobSetError>,
    },

    #[error("failed to read {path}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: PackageXmlError,
    },
}

/// Discover every ROS package nested under `workspace_root`.
///
/// Directories containing any of `COLCON_IGNORE`, `AMENT_IGNORE`, or
/// `CATKIN_IGNORE` are pruned from the walk. Colcon writes these markers into
/// its own `build/`, `install/`, and `log/` directories, so honoring the
/// markers also handles those without any directory-name hardcoding.
///
/// Returns an error if two `package.xml` files declare the same `<name>`.
pub fn discover_ros_packages(
    workspace_root: &Path,
) -> Result<WorkspaceDiscovery, WorkspaceDiscoveryError> {
    let _span = tracing::info_span!(
        "ros_workspace_discovery",
        workspace_root = %workspace_root.display(),
    )
    .entered();
    let started = std::time::Instant::now();

    let mut packages: HashMap<String, PathBuf> = HashMap::new();

    if workspace_root.is_dir() {
        let mut marker_filenames = Vec::with_capacity(IGNORE_MARKERS.len() + 1);
        marker_filenames.push(PACKAGE_XML);
        marker_filenames.extend(IGNORE_MARKERS.iter().copied());

        // Hidden directories (`.git`, `.pixi`, `.vscode`, ...) are skipped
        // by the walker; matches colcon's behaviour and avoids descending
        // into the installed `.pixi/envs/.../share/...` tree whose
        // `package.xml`s are not workspace siblings.
        let matches = GlobSet::create([format!("**/{PACKAGE_XML}").as_str()])
            .with_exclude_hidden(true)
            .with_ignore_marker_filenames(marker_filenames)
            .collect_matching(workspace_root)
            .map_err(|source| WorkspaceDiscoveryError::Walk {
                path: workspace_root.to_path_buf(),
                source: Box::new(source),
            })?;

        for matched in matches {
            let package_xml = matched.into_path();
            let content = fs::read_to_string(&package_xml).map_err(|source| {
                WorkspaceDiscoveryError::ReadFile {
                    path: package_xml.clone(),
                    source,
                }
            })?;
            let parsed =
                PackageXml::parse(&content).map_err(|source| WorkspaceDiscoveryError::Parse {
                    path: package_xml.clone(),
                    source,
                })?;

            if let Some(existing) = packages.get(&parsed.name) {
                return Err(WorkspaceDiscoveryError::DuplicateName {
                    name: parsed.name,
                    first: existing.clone(),
                    second: package_xml,
                });
            }
            packages.insert(parsed.name, package_xml);
        }
    }

    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        packages = packages.len(),
        "ROS workspace discovery finished"
    );

    let mut markers: Vec<String> = Vec::with_capacity(IGNORE_MARKERS.len() + 1);
    markers.push(PACKAGE_XML.to_string());
    markers.extend(IGNORE_MARKERS.iter().map(|m| m.to_string()));
    let input_glob_set = InputGlobSet {
        // Pixi manifests are included because a sibling that holds a pixi
        // package manifest is referenced by its directory instead of its
        // package.xml (see `sibling_source_spec`), so adding or editing
        // such a manifest changes the emitted metadata.
        patterns: vec![
            format!("**/{PACKAGE_XML}"),
            "**/pixi.toml".to_string(),
            "**/pyproject.toml".to_string(),
        ],
        markers,
        exclude_hidden: true,
        // Caller (pixi-build-ros::main) fills in the workspace root
        // before emitting the recipe.
        root: None,
    };

    Ok(WorkspaceDiscovery {
        packages,
        input_glob_set,
    })
}

fn path_to_forward_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Build a matchspec string of the form
/// `ros-<distro>-<name>[url="source://?path=<rel-to-manifest>"]` that, when
/// parsed via `SerializableMatchSpec::from`, yields a source dependency
/// pointing at the sibling package.
///
/// Siblings that hold a pixi package manifest are referenced by their
/// directory: directory discovery uses that manifest, including its
/// `[package.build.config]`, which an explicit `package.xml` reference
/// deliberately ignores. Bare ROS packages are referenced by their
/// `package.xml`, the only way to select ROS discovery for them.
pub fn sibling_source_spec(
    conda_name: &str,
    sibling_package_xml: &Path,
    manifest_root: &Path,
) -> String {
    let target = match sibling_package_xml.parent() {
        Some(dir) if pixi_build_discovery::is_pixi_package_directory(dir) => dir,
        _ => sibling_package_xml,
    };
    let rel = pathdiff::diff_paths(target, manifest_root).unwrap_or_else(|| target.to_path_buf());
    let rel = path_to_forward_slashes(&rel);

    let mut url = Url::from_str("source://").expect("static URL parses");
    url.query_pairs_mut().append_pair("path", &rel);
    format!("{conda_name}[url=\"{url}\"]")
}

/// Build the conda-formatted package name for a ROS package.
pub fn conda_name_for(distro: &str, ros_name: &str) -> String {
    format!("ros-{}-{}", distro, ros_name.replace('_', "-"))
}

/// Compute the `conda-name -> source-spec-string` map for every sibling
/// package discovered under the workspace, excluding the current package.
pub fn sibling_source_spec_map(
    discovered: &HashMap<String, PathBuf>,
    current_package_name: &str,
    manifest_root: &Path,
    distro_name: &str,
) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (ros_name, package_xml_path) in discovered {
        if ros_name == current_package_name {
            continue;
        }
        let conda_name = conda_name_for(distro_name, ros_name);
        let spec = sibling_source_spec(&conda_name, package_xml_path, manifest_root);
        map.insert(conda_name, spec);
    }
    map
}

/// Filter a sibling source-spec map down to entries whose conda name is **not**
/// already declared in `existing`. Used to honor the "manual entries in
/// pixi.toml always win over discovery" rule on a per-requirement-class basis.
pub fn filter_unspecified<'a>(
    overrides: &'a HashMap<String, String>,
    existing: &ConditionalList<SerializableMatchSpec>,
) -> HashMap<String, &'a str> {
    let declared: std::collections::HashSet<String> =
        existing.iter().filter_map(item_package_name).collect();
    overrides
        .iter()
        .filter(|(name, _)| !declared.contains(*name))
        .map(|(name, spec)| (name.clone(), spec.as_str()))
        .collect()
}

/// Replace every binary `Item` in `list` whose package name appears as a key
/// in `overrides` with a source `Item` built from the override's spec string.
/// Items that don't match are kept as-is.
pub fn apply_sibling_overrides(
    list: ConditionalList<SerializableMatchSpec>,
    overrides: &HashMap<String, &str>,
) -> ConditionalList<SerializableMatchSpec> {
    if overrides.is_empty() {
        return list;
    }
    let items: Vec<Item<SerializableMatchSpec>> = list
        .iter()
        .map(|item| {
            if let Some(name) = item_package_name(item)
                && let Some(source_spec) = overrides.get(&name)
            {
                Item::Value(Value::new_concrete(
                    SerializableMatchSpec::from(*source_spec),
                    None,
                ))
            } else {
                item.clone()
            }
        })
        .collect();
    ConditionalList::new(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err as fs;
    use tempfile::tempdir;

    fn write_package_xml(dir: &Path, name: &str) {
        fs::create_dir_all(dir).unwrap();
        let xml = format!(
            r#"<?xml version="1.0"?>
<package format="3">
  <name>{name}</name>
  <version>0.0.1</version>
  <description>test</description>
  <maintainer email="test@example.com">Tester</maintainer>
  <license>MIT</license>
</package>
"#
        );
        fs::write(dir.join("package.xml"), xml).unwrap();
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn discovers_top_level_packages() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_package_xml(&root.join("pkg_a"), "pkg_a");
        write_package_xml(&root.join("pkg_b"), "pkg_b");

        let result = discover_ros_packages(root).unwrap();

        assert_eq!(result.packages.len(), 2);
        assert_eq!(
            result.packages.get("pkg_a").unwrap(),
            &root.join("pkg_a").join("package.xml"),
        );
        assert_eq!(
            result.packages.get("pkg_b").unwrap(),
            &root.join("pkg_b").join("package.xml"),
        );
    }

    #[test]
    fn discovers_nested_packages() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_package_xml(&root.join("src").join("pkg_a"), "pkg_a");
        write_package_xml(&root.join("src").join("nested").join("pkg_b"), "pkg_b");

        let result = discover_ros_packages(root).unwrap();
        assert_eq!(result.packages.len(), 2);
        assert!(result.packages.contains_key("pkg_a"));
        assert!(result.packages.contains_key("pkg_b"));
    }

    #[test]
    fn colcon_ignore_prunes_subtree() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_package_xml(&root.join("pkg_a"), "pkg_a");
        // pkg_b is inside a directory with COLCON_IGNORE
        write_package_xml(&root.join("build").join("pkg_b"), "pkg_b");
        touch(&root.join("build").join("COLCON_IGNORE"));

        let result = discover_ros_packages(root).unwrap();
        assert_eq!(result.packages.len(), 1);
        assert!(result.packages.contains_key("pkg_a"));
        // Pixi re-runs the discovery with the same marker semantics to
        // learn that `build/` is pruned; the marker carries that intent.
        assert!(
            result
                .input_glob_set
                .markers
                .iter()
                .any(|m| m == "COLCON_IGNORE")
        );
    }

    #[test]
    fn ament_and_catkin_ignore_also_prune() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_package_xml(&root.join("a").join("pkg"), "in_ament");
        touch(&root.join("a").join("AMENT_IGNORE"));
        write_package_xml(&root.join("b").join("pkg"), "in_catkin");
        touch(&root.join("b").join("CATKIN_IGNORE"));
        write_package_xml(&root.join("c").join("pkg"), "visible");

        let result = discover_ros_packages(root).unwrap();
        assert_eq!(result.packages.len(), 1);
        assert!(result.packages.contains_key("visible"));
    }

    #[test]
    fn duplicate_name_is_an_error() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_package_xml(&root.join("a"), "same_name");
        write_package_xml(&root.join("b"), "same_name");

        let err = discover_ros_packages(root).unwrap_err();
        match err {
            WorkspaceDiscoveryError::DuplicateName { name, .. } => {
                assert_eq!(name, "same_name");
            }
            other => panic!("expected DuplicateName, got {other:?}"),
        }
    }

    #[test]
    fn empty_workspace_returns_no_packages_but_keeps_glob_set() {
        let tmp = tempdir().unwrap();
        let result = discover_ros_packages(tmp.path()).unwrap();
        assert!(result.packages.is_empty());
        assert_eq!(
            result.input_glob_set.patterns,
            vec![
                "**/package.xml".to_string(),
                "**/pixi.toml".to_string(),
                "**/pyproject.toml".to_string(),
            ]
        );
        assert!(
            result
                .input_glob_set
                .markers
                .iter()
                .any(|m| m == "COLCON_IGNORE"),
            "expected COLCON_IGNORE marker, got: {:?}",
            result.input_glob_set.markers
        );
        assert!(result.input_glob_set.exclude_hidden);
    }

    #[test]
    fn nonexistent_workspace_root_returns_empty() {
        let result = discover_ros_packages(Path::new("/this/path/does/not/exist")).unwrap();
        assert!(result.packages.is_empty());
    }

    #[test]
    fn sibling_with_pixi_package_manifest_is_referenced_by_directory() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();

        // The workspace the member packages belong to; manifest discovery for
        // a member walks up to this manifest.
        fs::write(
            root.join("pixi.toml"),
            r#"
[workspace]
name = "ws"
channels = ["conda-forge"]
platforms = ["linux-64"]
preview = ["pixi-build"]
"#,
        )
        .unwrap();

        write_package_xml(&root.join("launch_pkg"), "launch_pkg");
        write_package_xml(&root.join("node_pkg"), "node_pkg");
        write_package_xml(&root.join("plain_pkg"), "plain_pkg");
        fs::write(
            root.join("node_pkg").join("pixi.toml"),
            r#"
[package]
name = "ros-humble-node-pkg"
version = "0.0.0"

[package.build]
backend = { name = "pixi-build-ros", version = "*" }

[package.build.config]
extra-package-mappings = [{ "my-conda-dep" = { conda = ["xtensor"] } }]
"#,
        )
        .unwrap();

        let discovered = discover_ros_packages(root).unwrap().packages;
        let manifest_root = root.join("launch_pkg");
        let specs = sibling_source_spec_map(&discovered, "launch_pkg", &manifest_root, "humble");

        let spec_for = |rel: &str| {
            let mut url = Url::from_str("source://").unwrap();
            url.query_pairs_mut().append_pair("path", rel);
            url
        };

        // node_pkg carries its own pixi package manifest: reference the
        // directory so pixi discovers that manifest, including its
        // [package.build.config].
        assert_eq!(
            specs.get("ros-humble-node-pkg").unwrap(),
            &format!("ros-humble-node-pkg[url=\"{}\"]", spec_for("../node_pkg")),
        );

        // plain_pkg is a bare ROS package: keep pointing at its package.xml,
        // which is the only way to select ROS discovery for it.
        assert_eq!(
            specs.get("ros-humble-plain-pkg").unwrap(),
            &format!(
                "ros-humble-plain-pkg[url=\"{}\"]",
                spec_for("../plain_pkg/package.xml")
            ),
        );
    }

    #[test]
    fn hidden_directories_are_pruned() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_package_xml(&root.join("src").join("pkg_a"), "pkg_a");

        // Simulate the contents of a built `.pixi/envs` tree: installed
        // packages bring their own package.xml files which must not be
        // mistaken for sibling sources.
        write_package_xml(
            &root
                .join(".pixi")
                .join("envs")
                .join("default")
                .join("share")
                .join("foo"),
            "foo",
        );
        // Hidden dirs other than .pixi should also be skipped (matches colcon).
        write_package_xml(&root.join(".git").join("worktree").join("pkg_b"), "pkg_b");

        let result = discover_ros_packages(root).unwrap();
        assert_eq!(result.packages.len(), 1);
        assert!(result.packages.contains_key("pkg_a"));
        assert!(result.input_glob_set.exclude_hidden);
    }
}
