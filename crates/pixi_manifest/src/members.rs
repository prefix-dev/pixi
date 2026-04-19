//! Recursive downward discovery of nested member workspaces under a
//! workspace root.
//!
//! This module implements the structural discovery side of the
//! `hierarchical-tasks` preview feature described in issue
//! [#5003](https://github.com/prefix-dev/pixi/issues/5003).
//!
//! **Model 2 — federated member workspaces.** A "member" is a subdirectory
//! that contains its own pixi-compatible manifest (`pixi.toml`,
//! `pyproject.toml`, or `mojoproject.toml`) with a top-level `[workspace]`
//! block (or `[tool.pixi.workspace]` for pyproject). Members have their
//! own environments, channels, and lockfile — they are fully standalone
//! pixi projects. Running `pixi run test` inside a member directory
//! treats that member as the root workspace, without any knowledge of
//! the outer aggregation.
//!
//! This module produces only **structural metadata** — names and paths.
//! Actual loading of each member as a `pixi_core::Workspace` happens in a
//! later layer, keyed off the tree returned here.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use indexmap::IndexMap;
use miette::{Diagnostic, NamedSource};
use pixi_consts::consts;
use thiserror::Error;

use crate::{
    ManifestKind, ManifestProvenance, ProvenanceError, TomlError,
    utils::WithSourceCode,
};

/// A tree of member workspaces rooted under a workspace directory.
#[derive(Debug, Default, Clone)]
pub struct MemberTree {
    members: IndexMap<String, MemberNode>,
}

/// A single member node in the tree. Holds only structural metadata —
/// the member is loaded as a full `pixi_core::Workspace` later.
#[derive(Debug, Clone)]
pub struct MemberNode {
    /// The workspace name declared in this member's manifest (from
    /// `[workspace].name`, `[tool.pixi.workspace].name`, or `[project].name`
    /// as a pyproject fallback).
    pub name: String,
    /// Absolute path to the manifest file for this member.
    pub manifest_path: PathBuf,
    /// Absolute path to the directory containing the manifest.
    pub dir: PathBuf,
    /// Nested child members (unbounded depth).
    pub children: IndexMap<String, MemberNode>,
}

impl MemberTree {
    /// Returns true if no members were discovered.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The top-level members.
    pub fn members(&self) -> &IndexMap<String, MemberNode> {
        &self.members
    }

    /// Walks a member path (e.g. `["a", "c"]`) and returns the matching node,
    /// or `None` if any segment does not exist.
    pub fn resolve<I, S>(&self, path: I) -> Option<&MemberNode>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut iter = path.into_iter();
        let first = iter.next()?;
        let mut cursor = self.members.get(first.as_ref())?;
        for seg in iter {
            cursor = cursor.children.get(seg.as_ref())?;
        }
        Some(cursor)
    }

    /// Yields every reachable member as `(path_segments, node)` in
    /// depth-first, insertion order.
    pub fn walk(&self) -> Vec<(Vec<String>, &MemberNode)> {
        fn visit<'a>(
            prefix: &[String],
            members: &'a IndexMap<String, MemberNode>,
            out: &mut Vec<(Vec<String>, &'a MemberNode)>,
        ) {
            for node in members.values() {
                let mut path = prefix.to_vec();
                path.push(node.name.clone());
                out.push((path.clone(), node));
                visit(&path, &node.children, out);
            }
        }

        let mut out = Vec::new();
        visit(&[], &self.members, &mut out);
        out
    }
}

/// Errors returned by [`discover_members`].
#[derive(Debug, Error, Diagnostic)]
pub enum MemberDiscoveryError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(Box<WithSourceCode<TomlError, NamedSource<Arc<str>>>>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    ProvenanceError(#[from] ProvenanceError),

    #[error(
        "duplicate member workspace name `{name}`: found at `{first}` and `{second}`"
    )]
    DuplicateSibling {
        name: String,
        first: PathBuf,
        second: PathBuf,
    },
}

/// Discovers nested member workspaces rooted at `workspace_dir`.
///
/// Walks the directory tree under `workspace_dir`, skipping common build /
/// cache / VCS directories. When a directory contains a pixi-compatible
/// manifest with a `[workspace]` block that declares a name, that
/// directory becomes a member and descent continues inside it for further
/// nested members.
///
/// Manifests without `[workspace]` (for example, a `pixi.toml` that only
/// declares `[package]` or is otherwise a non-workspace manifest) are
/// transparent — discovery walks through them as if they weren't there
/// and keeps searching deeper.
///
/// Returns an empty tree when no members are found. The root manifest at
/// `workspace_dir/pixi.toml` (or equivalent) is never treated as a member —
/// only descendants are considered.
pub fn discover_members(workspace_dir: &Path) -> Result<MemberTree, MemberDiscoveryError> {
    let mut tree = MemberTree::default();
    walk(workspace_dir, &mut tree.members)?;
    Ok(tree)
}

fn walk(
    dir: &Path,
    members: &mut IndexMap<String, MemberNode>,
) -> Result<(), MemberDiscoveryError> {
    let mut entries: Vec<PathBuf> = fs_err::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| should_descend(p))
        .collect();
    entries.sort();

    for entry in entries {
        let Some(provenance) = provenance_from_dir(&entry) else {
            walk(&entry, members)?;
            continue;
        };

        let parsed = parse_member_manifest(&provenance)?;

        // Under Model 2, a directory is a member iff its manifest has
        // [workspace]. Anything else — including `[package]`-only
        // manifests — is transparent: we walk right through it to find
        // any deeper members.
        let Some(name) = parsed.workspace_name else {
            walk(&entry, members)?;
            continue;
        };

        // Recurse into this member for further nested members.
        let mut children = IndexMap::new();
        walk(&entry, &mut children)?;

        let node = MemberNode {
            name: name.clone(),
            manifest_path: provenance.path.clone(),
            dir: entry.clone(),
            children,
        };

        if let Some(existing) = members.get(&name) {
            return Err(MemberDiscoveryError::DuplicateSibling {
                name,
                first: existing.dir.clone(),
                second: entry,
            });
        }

        members.insert(name, node);
    }

    Ok(())
}

/// Structured view of a member manifest used during discovery.
///
/// `workspace_name` is `Some(name)` only when the manifest contains a
/// `[workspace]` block that declares a name. Anything else is `None`.
struct ParsedMember {
    workspace_name: Option<String>,
}

fn parse_member_manifest(
    provenance: &ManifestProvenance,
) -> Result<ParsedMember, MemberDiscoveryError> {
    let contents = provenance.read()?.map(Arc::<str>::from);
    let source_name = provenance.absolute_path().to_string_lossy().into_owned();
    let inner: Arc<str> = match &contents {
        crate::ManifestSource::PixiToml(s)
        | crate::ManifestSource::PyProjectToml(s)
        | crate::ManifestSource::MojoProjectToml(s) => s.clone(),
    };

    let toml = match toml_span::parse(inner.as_ref()) {
        Ok(t) => t,
        Err(e) => {
            let source = NamedSource::new(source_name, inner).with_language("toml");
            return Err(MemberDiscoveryError::Toml(Box::new(WithSourceCode {
                error: TomlError::from(e),
                source,
            })));
        }
    };

    let (workspace_ptr, workspace_name_ptr): (&'static str, &'static str) = match provenance.kind {
        ManifestKind::Pixi | ManifestKind::MojoProject => ("/workspace", "/workspace/name"),
        ManifestKind::Pyproject => ("/tool/pixi/workspace", "/tool/pixi/workspace/name"),
    };

    // `[workspace]` is the sole membership signal.
    if toml.pointer(workspace_ptr).is_none() {
        return Ok(ParsedMember { workspace_name: None });
    }

    let workspace_name = toml
        .pointer(workspace_name_ptr)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or_else(|| {
            if matches!(provenance.kind, ManifestKind::Pyproject) {
                toml.pointer("/project/name")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
            } else {
                None
            }
        });

    Ok(ParsedMember { workspace_name })
}

/// Directories we never descend into. Dot-prefixed names are skipped
/// separately in [`should_descend`].
const SKIP_DIR_NAMES: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "__pycache__",
    "venv",
];

fn should_descend(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    if name.starts_with('.') {
        return false;
    }
    !SKIP_DIR_NAMES.contains(&name)
}

fn provenance_from_dir(dir: &Path) -> Option<ManifestProvenance> {
    let pixi = dir.join(consts::WORKSPACE_MANIFEST);
    let pyproject = dir.join(consts::PYPROJECT_MANIFEST);
    let mojo = dir.join(consts::MOJOPROJECT_MANIFEST);
    if pixi.is_file() {
        Some(ManifestProvenance::new(pixi, ManifestKind::Pixi))
    } else if pyproject.is_file() {
        Some(ManifestProvenance::new(pyproject, ManifestKind::Pyproject))
    } else if mojo.is_file() {
        Some(ManifestProvenance::new(mojo, ManifestKind::MojoProject))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    /// Minimal valid `[workspace]` block for a member fixture.
    fn member_workspace_toml(name: &str) -> String {
        format!(
            "[workspace]\nname = \"{name}\"\nchannels = []\nplatforms = []\n"
        )
    }

    fn member_workspace_with_task(name: &str, task_body: &str) -> String {
        format!(
            "[workspace]\nname = \"{name}\"\nchannels = []\nplatforms = []\n\n[tasks]\n{task_body}\n"
        )
    }

    #[test]
    fn empty_workspace_no_members() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = discover_members(tmp.path()).unwrap();
        assert!(tree.is_empty());
    }

    #[test]
    fn discovers_top_level_members() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a/pixi.toml"), &member_workspace_toml("a"));
        write(&tmp.path().join("b/pixi.toml"), &member_workspace_toml("b"));

        let tree = discover_members(tmp.path()).unwrap();
        let names: Vec<_> = tree.members().keys().cloned().collect();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
        assert!(tree.resolve(["a"]).is_some());
        assert!(tree.resolve(["b"]).is_some());
        assert!(tree.resolve(["c"]).is_none());
    }

    #[test]
    fn discovers_nested_members() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a/pixi.toml"), &member_workspace_toml("a"));
        write(&tmp.path().join("a/c/pixi.toml"), &member_workspace_toml("c"));

        let tree = discover_members(tmp.path()).unwrap();
        let a = tree.resolve(["a"]).expect("a should exist");
        assert!(a.children.contains_key("c"));
        assert!(tree.resolve(["a", "c"]).is_some());
    }

    #[test]
    fn intermediate_dir_without_manifest_is_transparent() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("b/c/pixi.toml"), &member_workspace_toml("c"));

        let tree = discover_members(tmp.path()).unwrap();
        assert!(tree.resolve(["c"]).is_some());
        assert!(tree.resolve(["b"]).is_none());
    }

    #[test]
    fn package_only_manifest_is_transparent() {
        // A manifest with only [package] and no [workspace] is NOT a
        // member and discovery descends through it as if the manifest
        // weren't there.
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("tools/pixi.toml"),
            "[package]\nname = \"tools\"\nversion = \"0.1.0\"\n",
        );
        write(
            &tmp.path().join("tools/inner/pixi.toml"),
            &member_workspace_toml("inner"),
        );

        let tree = discover_members(tmp.path()).unwrap();
        assert!(tree.resolve(["inner"]).is_some());
        assert!(tree.resolve(["tools"]).is_none());
    }

    #[test]
    fn skips_common_build_and_hidden_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("target/pixi.toml"),
            &member_workspace_toml("should_not_appear"),
        );
        write(
            &tmp.path().join(".pixi/pixi.toml"),
            &member_workspace_toml("should_not_appear_either"),
        );
        write(
            &tmp.path().join("node_modules/pixi.toml"),
            &member_workspace_toml("also_nope"),
        );
        write(&tmp.path().join("ok/pixi.toml"), &member_workspace_toml("ok"));

        let tree = discover_members(tmp.path()).unwrap();
        let names: Vec<_> = tree.members().keys().cloned().collect();
        assert_eq!(names, vec!["ok".to_string()]);
    }

    #[test]
    fn duplicate_sibling_name_errors() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a/pixi.toml"), &member_workspace_toml("same"));
        write(&tmp.path().join("b/pixi.toml"), &member_workspace_toml("same"));

        let err = discover_members(tmp.path()).unwrap_err();
        assert!(
            matches!(err, MemberDiscoveryError::DuplicateSibling { ref name, .. } if name == "same")
        );
    }

    #[test]
    fn nested_workspaces_are_expected() {
        // A member may itself contain members with [workspace]. This is
        // the expected Model-2 shape; no error is returned.
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a/pixi.toml"), &member_workspace_toml("a"));
        write(&tmp.path().join("a/c/pixi.toml"), &member_workspace_toml("c"));
        let tree = discover_members(tmp.path()).unwrap();
        assert!(tree.resolve(["a", "c"]).is_some());
    }

    #[test]
    fn deeply_nested_transparent_chain() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a/pixi.toml"), &member_workspace_toml("a"));
        // mid has no manifest; c sits under a/mid/c
        write(&tmp.path().join("a/mid/c/pixi.toml"), &member_workspace_toml("c"));

        let tree = discover_members(tmp.path()).unwrap();
        assert!(tree.resolve(["a", "c"]).is_some());
    }

    #[test]
    fn walk_yields_paths_in_insertion_order() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a/pixi.toml"), &member_workspace_toml("a"));
        write(&tmp.path().join("a/c/pixi.toml"), &member_workspace_toml("c"));
        write(&tmp.path().join("b/pixi.toml"), &member_workspace_toml("b"));

        let tree = discover_members(tmp.path()).unwrap();
        let paths: Vec<Vec<String>> = tree.walk().into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            paths,
            vec![
                vec!["a".to_string()],
                vec!["a".to_string(), "c".to_string()],
                vec!["b".to_string()],
            ]
        );
    }

    #[test]
    fn member_with_tasks_still_discovered_structurally() {
        // Discovery doesn't parse or return tasks — task extraction
        // happens during Workspace loading in pixi_core. This test just
        // confirms a tasks block doesn't confuse discovery.
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("a/pixi.toml"),
            &member_workspace_with_task("a", "greet = \"echo hi\""),
        );

        let tree = discover_members(tmp.path()).unwrap();
        assert!(tree.resolve(["a"]).is_some());
    }
}
