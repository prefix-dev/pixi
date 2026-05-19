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
use rattler_build_recipe::stage0::{ConditionalList, Item, SerializableMatchSpec, Value};
use thiserror::Error;
use url::Url;

use crate::package_map::item_package_name;
use crate::package_xml::{PackageXml, PackageXmlError};

const IGNORE_MARKERS: &[&str] = &["COLCON_IGNORE", "AMENT_IGNORE", "CATKIN_IGNORE"];

/// Result of walking a workspace root for ROS packages.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceDiscovery {
    /// Map from the `<name>` of each discovered package to the absolute path of
    /// its `package.xml`.
    pub packages: HashMap<String, PathBuf>,
    /// Gitignore-style glob patterns relative to the workspace root.
    ///
    /// Layout is "includes first, then exclusions" so pixi's last-match-wins
    /// matcher correctly cancels the discovery-include in pruned subtrees.
    /// Callers that mix these globs with other patterns (for example a
    /// metadata-provider's anchored `setup.py` or a user's `**/*.urdf`)
    /// should append those other patterns *after* the list returned here, so
    /// they aren't suppressed by the broad `!<dir>/**` exclusions.
    pub input_globs: Vec<String>,
}

#[derive(Debug, Error, Diagnostic)]
pub enum WorkspaceDiscoveryError {
    #[error(
        "duplicate ROS package name `{name}` declared by both:\n  - {first}\n  - {second}"
    )]
    #[diagnostic(help(
        "Each ROS package in a workspace must have a unique <name> in its package.xml."
    ))]
    DuplicateName {
        name: String,
        first: PathBuf,
        second: PathBuf,
    },

    #[error("failed to read directory entry under {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
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
    let mut pruned_dirs: Vec<PathBuf> = Vec::new();

    if workspace_root.is_dir() {
        walk(workspace_root, workspace_root, &mut packages, &mut pruned_dirs)?;
    }

    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        packages = packages.len(),
        pruned_dirs = pruned_dirs.len(),
        "ROS workspace discovery finished"
    );

    let mut input_globs = vec![
        "**/package.xml".to_string(),
        "**/COLCON_IGNORE".to_string(),
        "**/AMENT_IGNORE".to_string(),
        "**/CATKIN_IGNORE".to_string(),
    ];
    // Skip the contents of any dot-prefixed directory in one shot, so we do
    // not have to enumerate every `.git` / `.pixi` / `.vscode` / etc. that
    // happens to live under the workspace. Marker-bearing dirs (which may or
    // may not be hidden) still get their own specific exclusion below.
    input_globs.push("!**/.*/**".to_string());
    for dir in pruned_dirs {
        input_globs.push(format!("!{}/**", path_to_forward_slashes(&dir)));
    }

    Ok(WorkspaceDiscovery {
        packages,
        input_globs,
    })
}

fn walk(
    workspace_root: &Path,
    dir: &Path,
    packages: &mut HashMap<String, PathBuf>,
    pruned_dirs: &mut Vec<PathBuf>,
) -> Result<(), WorkspaceDiscoveryError> {
    if IGNORE_MARKERS.iter().any(|m| dir.join(m).is_file()) {
        record_pruned(workspace_root, dir, pruned_dirs);
        return Ok(());
    }

    let package_xml = dir.join("package.xml");
    if package_xml.is_file() {
        let content =
            fs::read_to_string(&package_xml).map_err(|source| WorkspaceDiscoveryError::ReadFile {
                path: package_xml.clone(),
                source: source.into(),
            })?;
        let parsed = PackageXml::parse(&content).map_err(|source| WorkspaceDiscoveryError::Parse {
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

    let entries = fs::read_dir(dir).map_err(|source| WorkspaceDiscoveryError::Io {
        path: dir.to_path_buf(),
        source: source.into(),
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| WorkspaceDiscoveryError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Skip dot-prefixed directories the same way colcon does: this covers
        // `.pixi`, `.git`, `.vscode`, etc. without hardcoding any specific
        // name. The check is only applied to subdirectories, never to the
        // workspace root itself. A single `!**/.*/**` glob (emitted by the
        // caller) covers the invalidation side for all of these without
        // needing to enumerate them, so we do not record each one here.
        if is_hidden_dir(&path) {
            continue;
        }
        walk(workspace_root, &path, packages, pruned_dirs)?;
    }

    Ok(())
}

fn is_hidden_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn record_pruned(workspace_root: &Path, dir: &Path, pruned_dirs: &mut Vec<PathBuf>) {
    if let Some(rel) = pathdiff::diff_paths(dir, workspace_root)
        && !rel.as_os_str().is_empty()
    {
        pruned_dirs.push(rel);
    }
}

fn path_to_forward_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Build a matchspec string of the form
/// `ros-<distro>-<name>[url="source://?path=<rel-to-manifest>"]` that, when
/// parsed via `SerializableMatchSpec::from`, yields a source dependency
/// pointing at the sibling's `package.xml`.
pub fn sibling_source_spec(conda_name: &str, sibling_package_xml: &Path, manifest_root: &Path) -> String {
    let rel = pathdiff::diff_paths(sibling_package_xml, manifest_root)
        .unwrap_or_else(|| sibling_package_xml.to_path_buf());
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

/// Rewrite a discovery glob (relative to the workspace root) so it can be
/// evaluated from the package's `manifest_root`. The glob system rebases
/// `../`-style up-traversal patterns to a shared search root, so prefixing
/// each pattern with the relative path from `manifest_root` to
/// `workspace_root` is enough.
pub fn rewrite_glob_for_manifest_root(glob: &str, workspace_root: &Path, manifest_root: &Path) -> String {
    let rel = match pathdiff::diff_paths(workspace_root, manifest_root) {
        Some(p) if !p.as_os_str().is_empty() => path_to_forward_slashes(&p),
        _ => return glob.to_string(),
    };
    if let Some(rest) = glob.strip_prefix('!') {
        format!("!{rel}/{rest}")
    } else {
        format!("{rel}/{glob}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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
        assert!(result.input_globs.iter().any(|g| g == "!build/**"));
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
    fn empty_workspace_returns_no_packages_but_keeps_globs() {
        let tmp = tempdir().unwrap();
        let result = discover_ros_packages(tmp.path()).unwrap();
        assert!(result.packages.is_empty());
        assert!(result.input_globs.iter().any(|g| g == "**/package.xml"));
        assert!(result.input_globs.iter().any(|g| g == "**/COLCON_IGNORE"));
    }

    #[test]
    fn nonexistent_workspace_root_returns_empty() {
        let result = discover_ros_packages(Path::new("/this/path/does/not/exist")).unwrap();
        assert!(result.packages.is_empty());
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
        let contains = |needle: &str| result.input_globs.iter().any(|g| g == needle);
        assert!(
            contains("!**/.*/**"),
            "expected the single hidden-dir exclusion glob, got: {:?}",
            result.input_globs
        );
        // Per-hidden-dir exclusions are no longer emitted: the static
        // `!**/.*/**` covers all of them in one shot.
        assert!(
            !contains("!.pixi/**"),
            "per-hidden-dir exclusion must not be emitted any more, got: {:?}",
            result.input_globs
        );
        assert!(
            !contains("!.git/**"),
            "per-hidden-dir exclusion must not be emitted any more, got: {:?}",
            result.input_globs
        );
    }

    /// Exclusions must come after the includes so pixi's last-match-wins
    /// matcher actually cancels the includes inside pruned subtrees.
    #[test]
    fn input_globs_emit_includes_before_excludes() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_package_xml(&root.join("pkg"), "pkg");
        touch(&root.join("build").join("COLCON_IGNORE"));

        let result = discover_ros_packages(root).unwrap();
        let first_exclude = result
            .input_globs
            .iter()
            .position(|g| g.starts_with('!'))
            .expect("expected at least one exclusion");
        let last_include = result
            .input_globs
            .iter()
            .rposition(|g| !g.starts_with('!'))
            .expect("expected at least one include");
        assert!(
            last_include < first_exclude,
            "all includes must come before any exclusion, got: {:?}",
            result.input_globs
        );
    }
}
