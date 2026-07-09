//! Discovery of the publish set for `pixi publish`.
//!
//! A workspace-wide `pixi publish` operates on every package in the workspace
//! that opts into publishing with `publish = true` in its `[package]`
//! section. Package manifests are discovered by walking the workspace
//! directory tree; the walk respects ignore files (such as `.gitignore`),
//! skips hidden directories, and skips subtrees that belong to a nested
//! workspace.
//!
//! Every publish is a closed batch: each source dependency (build, host, or
//! run) of every published package must itself be part of the batch. Source
//! dependencies that do not opt into publishing - whether they point at a
//! directory inside the workspace or at something external (git or url
//! sources, paths escaping the workspace root) - fail the publish.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use miette::{Context, IntoDiagnostic};
use pixi_build_types::{PackageSpec, SourcePackageSpec, procedures::conda_outputs::CondaOutput};
use pixi_command_dispatcher::{
    BuildBackendMetadataSpec, CommandDispatcher, build::conversion::from_source_spec_v1,
};
use pixi_consts::consts::{MOJOPROJECT_MANIFEST, PYPROJECT_MANIFEST, WORKSPACE_MANIFEST};
use pixi_core::Workspace;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec};
use pixi_spec::{SourceAnchor, SourceLocationSpec};
use typed_path::Utf8TypedPathBuf;

/// The packages a workspace-wide `pixi publish` operates on, plus diagnostics
/// gathered while resolving them.
pub(crate) struct WorkspacePackageSet {
    /// Workspace-relative package sources, ordered so that every package
    /// appears after the packages it depends on (dependencies first). Upload
    /// in this order never leaves the target channel with a package whose
    /// run dependencies are missing.
    pub packages: Vec<PinnedSourceSpec>,

    /// Members that had to be forced out of a dependency cycle. Uploads
    /// involving these packages cannot be fully dependency-ordered.
    pub cycle_members: Vec<String>,
}

/// Where a source dependency points, relative to the workspace.
enum SourceTarget {
    /// A workspace-relative directory.
    Member(String),

    /// A source that lives outside the workspace (git/url source or a path
    /// escaping the workspace root).
    External(String),
}

/// Resolve the set of packages that opt into publishing with
/// `publish = true` and return them in dependency order (dependencies before
/// dependents).
///
/// `make_metadata_spec` builds the per-package metadata request; the caller
/// owns the environment/channel/variant configuration that goes into it. The
/// metadata computed here is cached by the command dispatcher, so the
/// subsequent build phase does not pay for it twice.
pub(crate) async fn resolve_publish_set(
    workspace: &Workspace,
    command_dispatcher: &CommandDispatcher,
    make_metadata_spec: impl Fn(PinnedSourceSpec) -> BuildBackendMetadataSpec,
) -> miette::Result<WorkspacePackageSet> {
    let workspace_root = workspace.root();

    let members = discover_opted_in_packages(workspace_root)?;
    if members.is_empty() {
        return Err(miette::diagnostic!(
            help = "Set `publish = true` in the `[package]` section of every package that \
                    `pixi publish` should publish. To publish a single package, pass \
                    `--path <dir>`.",
            "no package in the workspace opts into publishing",
        )
        .into());
    }

    // Query each package's build backend metadata and validate the closure:
    // every source dependency must itself opt into publishing. Output names
    // must be unique across the set; the target cannot hold two different
    // packages under the same name.
    let mut member_dependencies: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut output_owners: BTreeMap<String, String> = BTreeMap::new();
    let mut violations: BTreeSet<String> = BTreeSet::new();
    for dir in &members {
        let manifest_source: PinnedSourceSpec = PinnedPathSpec {
            path: dir.clone().into(),
        }
        .into();
        let backend_metadata = command_dispatcher
            .build_backend_metadata(make_metadata_spec(manifest_source.clone()))
            .await?;

        // Relative path specs in the metadata are anchored to the package
        // that declares them, and resolve to workspace-relative paths.
        let anchor = SourceAnchor::from(SourceLocationSpec::from(manifest_source));

        for output in &backend_metadata.metadata.outputs {
            let output_name = output.metadata.name.as_normalized().to_string();
            if let Some(owner) = output_owners.get(&output_name) {
                if owner != dir {
                    return Err(miette::diagnostic!(
                        help = "Packages in the publish set must have unique names. Remove \
                                `publish = true` from one of the two packages.",
                        "packages '{owner}' and '{dir}' both produce an output named '{output_name}'",
                    )
                    .into());
                }
            } else {
                output_owners.insert(output_name, dir.clone());
            }

            for (_name, source_spec) in output_source_dependencies(output) {
                let resolved = anchor.resolve(from_source_spec_v1(source_spec.clone()));
                match classify_source_location(workspace_root, resolved.location) {
                    SourceTarget::Member(dep_dir) => {
                        if members.contains(&dep_dir) {
                            if dep_dir != *dir {
                                member_dependencies
                                    .entry(dir.clone())
                                    .or_default()
                                    .insert(dep_dir);
                            }
                        } else {
                            violations.insert(format!(
                                "package '{dir}' depends on '{dep_dir}', which is not part of the publish set"
                            ));
                        }
                    }
                    SourceTarget::External(location) => {
                        violations.insert(format!(
                            "package '{dir}' depends on '{location}', which lives outside the workspace"
                        ));
                    }
                }
            }
        }
    }

    if !violations.is_empty() {
        return Err(miette::diagnostic!(
            help = "Every source dependency of a published package must itself be published \
                    in the same batch. Set `publish = true` in the `[package]` section of \
                    the missing packages, or replace the source dependencies with binary \
                    dependencies.",
            "the packages cannot be published as a self-contained set:\n  {}",
            violations.iter().cloned().collect::<Vec<_>>().join("\n  "),
        )
        .into());
    }

    let ordering = dependency_order(&members, &member_dependencies);
    let packages = ordering
        .order
        .into_iter()
        .map(|dir| PinnedPathSpec { path: dir.into() }.into())
        .collect();

    Ok(WorkspacePackageSet {
        packages,
        cycle_members: ordering.cycle_members,
    })
}

/// All source dependencies of a backend output as `(name, spec)` pairs:
/// build, host, and run dependencies plus the dependencies of every extra
/// group.
pub(super) fn output_source_dependencies(
    output: &CondaOutput,
) -> impl Iterator<Item = (&str, &SourcePackageSpec)> {
    let dependency_sets = output
        .build_dependencies
        .iter()
        .chain(output.host_dependencies.iter())
        .chain(std::iter::once(&output.run_dependencies));
    dependency_sets
        .flat_map(|deps| deps.depends.iter())
        .chain(output.extra_dependencies.values().flatten())
        .filter_map(|named| match &named.spec {
            PackageSpec::Source(source) => Some((named.name.as_str(), source)),
            PackageSpec::Binary(_) | PackageSpec::PinCompatible(_) => None,
        })
}

/// The publish-relevant contents of a manifest, read with a lightweight TOML
/// peek instead of a full manifest parse.
struct ManifestPeek {
    /// The manifest declares a workspace section (`[workspace]`, or the
    /// legacy `[project]` alias).
    declares_workspace: bool,

    /// The value of `publish` in the package section, if the manifest
    /// declares one.
    publish: Option<bool>,
}

/// A pixi manifest found while walking the workspace directory tree.
struct DiscoveredManifest {
    /// Absolute directory containing the manifest.
    dir: PathBuf,
    peek: ManifestPeek,
}

/// Read the publish-relevant keys of a manifest. For pyproject manifests the
/// pixi sections live under `[tool.pixi]`.
fn peek_manifest(manifest_path: &Path) -> miette::Result<ManifestPeek> {
    let source = fs_err::read_to_string(manifest_path).into_diagnostic()?;
    let doc = source
        .parse::<toml_edit::DocumentMut>()
        .into_diagnostic()
        .with_context(|| format!("failed to parse '{}'", manifest_path.display()))?;

    let is_pyproject = manifest_path
        .file_name()
        .is_some_and(|name| name == PYPROJECT_MANIFEST);
    let root: Option<&toml_edit::Item> = if is_pyproject {
        doc.get("tool").and_then(|tool| tool.get("pixi"))
    } else {
        Some(doc.as_item())
    };
    let Some(root) = root else {
        return Ok(ManifestPeek {
            declares_workspace: false,
            publish: None,
        });
    };

    let declares_workspace = root.get("workspace").is_some() || root.get("project").is_some();
    let publish = match root
        .get("package")
        .and_then(|package| package.get("publish"))
    {
        None => None,
        Some(item) => Some(item.as_bool().ok_or_else(|| {
            miette::diagnostic!(
                "the `publish` key of the package in '{}' must be a boolean",
                manifest_path.display(),
            )
        })?),
    };

    Ok(ManifestPeek {
        declares_workspace,
        publish,
    })
}

/// Precedence of manifest files within one directory: a `pixi.toml` shadows a
/// `pyproject.toml`, which shadows a `mojoproject.toml`.
fn manifest_priority(file_name: &str) -> Option<usize> {
    [WORKSPACE_MANIFEST, PYPROJECT_MANIFEST, MOJOPROJECT_MANIFEST]
        .iter()
        .position(|name| *name == file_name)
}

/// Walk the workspace directory tree and collect the member directories of
/// every package that opts into publishing with `publish = true`.
///
/// The walk respects ignore files (such as `.gitignore`) and skips hidden
/// directories. A manifest in a subdirectory that declares its own workspace
/// starts a nested, independent workspace: its entire subtree is skipped.
fn discover_opted_in_packages(workspace_root: &Path) -> miette::Result<BTreeSet<String>> {
    // The highest-priority manifest of every visited directory.
    let mut manifest_paths: BTreeMap<PathBuf, (usize, PathBuf)> = BTreeMap::new();
    for entry in ignore::WalkBuilder::new(workspace_root)
        .require_git(false)
        .build()
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::debug!("skipping unreadable entry while discovering packages: {error}");
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Some(priority) = entry.file_name().to_str().and_then(manifest_priority) else {
            continue;
        };
        let Some(dir) = entry.path().parent() else {
            continue;
        };
        match manifest_paths.entry(dir.to_path_buf()) {
            std::collections::btree_map::Entry::Vacant(vacant) => {
                vacant.insert((priority, entry.path().to_path_buf()));
            }
            std::collections::btree_map::Entry::Occupied(mut occupied) => {
                if priority < occupied.get().0 {
                    occupied.insert((priority, entry.path().to_path_buf()));
                }
            }
        }
    }

    let mut discovered = Vec::with_capacity(manifest_paths.len());
    for (dir, (_priority, manifest_path)) in manifest_paths {
        discovered.push(DiscoveredManifest {
            dir,
            peek: peek_manifest(&manifest_path)?,
        });
    }

    // A workspace declaration below the root starts a nested workspace; its
    // packages opt into publishing for that workspace, not for this one.
    let nested_workspace_roots: Vec<PathBuf> = discovered
        .iter()
        .filter(|manifest| manifest.peek.declares_workspace && manifest.dir != workspace_root)
        .map(|manifest| manifest.dir.clone())
        .collect();

    let mut members = BTreeSet::new();
    for manifest in discovered {
        if manifest.peek.publish != Some(true) {
            continue;
        }
        if nested_workspace_roots
            .iter()
            .any(|nested_root| manifest.dir.starts_with(nested_root))
        {
            continue;
        }
        let Some(relative) = pathdiff::diff_paths(&manifest.dir, workspace_root) else {
            continue;
        };
        members.insert(member_dir_from_relative_path(&relative));
    }

    Ok(members)
}

/// Canonical member-directory form of a workspace-root-relative filesystem
/// path. The workspace root itself is represented as `.`.
fn member_dir_from_relative_path(path: &Path) -> String {
    normalize_member_dir(native_relative_path_to_member(path))
}

/// Convert a relative filesystem path into the forward-slash form used to
/// identify members, so the same directory referenced through a manifest path
/// spec and through a filesystem path (backslash-separated on Windows)
/// compares equal.
fn native_relative_path_to_member(path: &Path) -> Utf8TypedPathBuf {
    Utf8TypedPathBuf::from(
        path.to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    )
}

/// Determine whether a workspace-anchored source location refers to a
/// directory inside the workspace or to something external.
fn classify_source_location(workspace_root: &Path, location: SourceLocationSpec) -> SourceTarget {
    let SourceLocationSpec::Path(path_spec) = &location else {
        return SourceTarget::External(location.to_string());
    };

    let path = path_spec.path.to_path();
    if path.is_absolute() || path.starts_with("~") {
        // Absolute paths can still point into the workspace; try to re-anchor
        // them. `~` cannot be resolved without the filesystem, treat it as
        // external.
        let std_path = Path::new(path_spec.path.as_str());
        let (Ok(canonical_path), Ok(canonical_root)) = (
            dunce::canonicalize(std_path),
            dunce::canonicalize(workspace_root),
        ) else {
            return SourceTarget::External(location.to_string());
        };
        return match canonical_path.strip_prefix(&canonical_root) {
            Ok(relative) => SourceTarget::Member(member_dir_for(
                workspace_root,
                native_relative_path_to_member(relative),
            )),
            Err(_) => SourceTarget::External(location.to_string()),
        };
    }

    // Workspace-relative path. A normalized path that still starts with `..`
    // escapes the workspace root.
    let as_str = path_spec.path.as_str();
    if as_str == ".." || as_str.starts_with("../") || as_str.starts_with("..\\") {
        return SourceTarget::External(location.to_string());
    }

    SourceTarget::Member(member_dir_for(workspace_root, path_spec.path.clone()))
}

/// Reduce a workspace-relative source path to the directory that identifies
/// the member package: paths that point at a manifest file are replaced by
/// their parent directory so that `pkg/pixi.toml` and `pkg` name the same
/// member.
fn member_dir_for(workspace_root: &Path, path: Utf8TypedPathBuf) -> String {
    let is_file = workspace_root.join(path.as_str()).is_file();
    let dir = if is_file {
        path.to_path()
            .parent()
            .map(|parent| parent.to_path_buf())
            .unwrap_or(path)
    } else {
        path
    };
    normalize_member_dir(dir)
}

/// Canonical string form of a member directory. The workspace root itself is
/// represented as `.`.
fn normalize_member_dir(dir: Utf8TypedPathBuf) -> String {
    let dir = dir.as_str().trim_end_matches(['/', '\\']);
    if dir.is_empty() || dir == "." {
        ".".to_string()
    } else {
        dir.to_string()
    }
}

/// The result of ordering members by their dependencies.
pub(super) struct DependencyOrdering {
    /// Members with dependencies before their dependents.
    pub order: Vec<String>,

    /// Members that were forced out of a dependency cycle. When this is
    /// non-empty the order is best-effort: a forced member is emitted before
    /// its dependencies are complete.
    pub cycle_members: Vec<String>,
}

/// Order members so that dependencies come before their dependents. Should a
/// dependency cycle exist, one member of the cycle is forced out instead of
/// failing; members that merely depend on the cycle still come after it. The
/// forced members are reported so the caller can warn the user.
///
/// Also used by the publish flow to order the outputs of a single
/// multi-output package.
pub(super) fn dependency_order(
    members: &BTreeSet<String>,
    dependencies: &BTreeMap<String, BTreeSet<String>>,
) -> DependencyOrdering {
    let mut order = Vec::with_capacity(members.len());
    let mut cycle_members = Vec::new();
    let mut emitted: BTreeSet<String> = BTreeSet::new();
    let mut remaining: BTreeSet<String> = members.clone();

    while !remaining.is_empty() {
        let ready: Vec<String> = remaining
            .iter()
            .filter(|member| {
                dependencies.get(*member).is_none_or(|deps| {
                    deps.iter()
                        .all(|dep| emitted.contains(dep) || !members.contains(dep))
                })
            })
            .cloned()
            .collect();

        if ready.is_empty() {
            let member = find_cycle_member(&remaining, dependencies);
            remaining.remove(&member);
            emitted.insert(member.clone());
            cycle_members.push(member.clone());
            order.push(member);
            continue;
        }

        for member in ready {
            remaining.remove(&member);
            emitted.insert(member.clone());
            order.push(member);
        }
    }

    DependencyOrdering {
        order,
        cycle_members,
    }
}

/// Pick a member that lies on a dependency cycle among `remaining`.
///
/// Walks unmet dependencies (always the lexicographically first one, for
/// determinism) starting from the first remaining member; the first member
/// visited twice is on a cycle. Only called when no member is ready, which
/// guarantees every walk eventually revisits a member.
fn find_cycle_member(
    remaining: &BTreeSet<String>,
    dependencies: &BTreeMap<String, BTreeSet<String>>,
) -> String {
    let mut current = remaining
        .first()
        .expect("caller ensures remaining is not empty")
        .clone();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    visited.insert(current.clone());
    loop {
        let next = dependencies
            .get(&current)
            .and_then(|deps| deps.iter().find(|dep| remaining.contains(*dep)))
            .cloned();
        match next {
            // No unmet dependency: the walk cannot continue, treat the
            // current member as the one to force out.
            None => return current,
            Some(next) => {
                if !visited.insert(next.clone()) {
                    return next;
                }
                current = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn write_manifest(root: &Path, dir: &str, contents: &str) {
        let dir = root.join(dir);
        fs_err::create_dir_all(&dir).unwrap();
        fs_err::write(dir.join("pixi.toml"), contents).unwrap();
    }

    #[test]
    fn dependency_order_puts_dependencies_first() {
        let members = set(&["kit", "core", "cpp"]);
        let mut deps = BTreeMap::new();
        deps.insert("kit".to_string(), set(&["cpp"]));
        deps.insert("cpp".to_string(), set(&["core"]));

        let ordering = dependency_order(&members, &deps);
        assert_eq!(ordering.order, vec!["core", "cpp", "kit"]);
        assert!(ordering.cycle_members.is_empty());
    }

    #[test]
    fn dependency_order_ignores_non_member_dependencies() {
        let members = set(&["a"]);
        let mut deps = BTreeMap::new();
        deps.insert("a".to_string(), set(&["not-a-member"]));

        let ordering = dependency_order(&members, &deps);
        assert_eq!(ordering.order, vec!["a"]);
        assert!(ordering.cycle_members.is_empty());
    }

    #[test]
    fn dependency_order_survives_cycles() {
        let members = set(&["a", "b", "c"]);
        let mut deps = BTreeMap::new();
        deps.insert("a".to_string(), set(&["b"]));
        deps.insert("b".to_string(), set(&["a"]));
        deps.insert("c".to_string(), set(&["a"]));

        let ordering = dependency_order(&members, &deps);
        // One cycle member is forced out, then the rest resolves normally;
        // `c` only depends on `a` and follows it.
        assert_eq!(ordering.order, vec!["a", "b", "c"]);
        assert_eq!(ordering.cycle_members, vec!["a"]);
    }

    #[test]
    fn dependency_order_emits_cycle_before_its_dependents() {
        // `0-app` sorts before the cycle members but merely depends on the
        // cycle; it must still be emitted after its dependency `a`.
        let members = set(&["0-app", "a", "b"]);
        let mut deps = BTreeMap::new();
        deps.insert("0-app".to_string(), set(&["a"]));
        deps.insert("a".to_string(), set(&["b"]));
        deps.insert("b".to_string(), set(&["a"]));

        let ordering = dependency_order(&members, &deps);
        // `a` is forced out of the cycle first; `0-app` and `b` both become
        // ready and follow in name order.
        assert_eq!(ordering.order, vec!["a", "0-app", "b"]);
        assert_eq!(ordering.cycle_members, vec!["a"]);
    }

    #[test]
    fn normalize_member_dir_maps_root_to_dot() {
        assert_eq!(normalize_member_dir(Utf8TypedPathBuf::from("")), ".");
        assert_eq!(normalize_member_dir(Utf8TypedPathBuf::from(".")), ".");
        assert_eq!(normalize_member_dir(Utf8TypedPathBuf::from("pkg/")), "pkg");
    }

    #[test]
    fn discovery_collects_only_packages_that_opt_in() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path();

        write_manifest(
            root_path,
            ".",
            "[workspace]\nchannels = []\n\n[package]\nname = \"root\"\npublish = true\n",
        );
        write_manifest(
            root_path,
            "packages/foo",
            "[package]\nname = \"foo\"\npublish = true\n",
        );
        write_manifest(
            root_path,
            "packages/bar",
            "[package]\nname = \"bar\"\npublish = false\n",
        );
        write_manifest(root_path, "packages/baz", "[package]\nname = \"baz\"\n");

        let members = discover_opted_in_packages(root_path).unwrap();
        assert_eq!(members, set(&[".", "packages/foo"]));
    }

    #[test]
    fn discovery_skips_nested_workspaces() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path();

        write_manifest(root_path, ".", "[workspace]\nchannels = []\n");
        write_manifest(
            root_path,
            "packages/foo",
            "[package]\nname = \"foo\"\npublish = true\n",
        );
        write_manifest(
            root_path,
            "examples/demo",
            "[workspace]\nchannels = []\n\n[package]\nname = \"demo\"\npublish = true\n",
        );
        write_manifest(
            root_path,
            "examples/demo/member",
            "[package]\nname = \"member\"\npublish = true\n",
        );

        let members = discover_opted_in_packages(root_path).unwrap();
        assert_eq!(members, set(&["packages/foo"]));
    }

    #[test]
    fn discovery_respects_ignore_files() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path();

        write_manifest(root_path, ".", "[workspace]\nchannels = []\n");
        write_manifest(
            root_path,
            "packages/foo",
            "[package]\nname = \"foo\"\npublish = true\n",
        );
        write_manifest(
            root_path,
            "vendored/dep",
            "[package]\nname = \"dep\"\npublish = true\n",
        );
        fs_err::write(root_path.join(".gitignore"), "vendored/\n").unwrap();

        let members = discover_opted_in_packages(root_path).unwrap();
        assert_eq!(members, set(&["packages/foo"]));
    }

    #[test]
    fn discovery_reads_pyproject_manifests() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path();

        write_manifest(root_path, ".", "[workspace]\nchannels = []\n");
        let dir = root_path.join("packages/py");
        fs_err::create_dir_all(&dir).unwrap();
        fs_err::write(
            dir.join("pyproject.toml"),
            "[tool.pixi.package]\nname = \"py\"\npublish = true\n",
        )
        .unwrap();

        let members = discover_opted_in_packages(root_path).unwrap();
        assert_eq!(members, set(&["packages/py"]));
    }

    #[test]
    fn discovery_rejects_non_boolean_publish() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path();

        write_manifest(root_path, ".", "[workspace]\nchannels = []\n");
        write_manifest(
            root_path,
            "packages/foo",
            "[package]\nname = \"foo\"\npublish = \"yes\"\n",
        );

        let err = discover_opted_in_packages(root_path).unwrap_err();
        assert!(err.to_string().contains("must be a boolean"));
    }
}
