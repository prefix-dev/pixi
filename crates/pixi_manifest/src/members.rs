//! Recursive downward discovery of nested member packages under a workspace
//! root.
//!
//! This module implements part of the `hierarchical-tasks` preview feature
//! described in issue [#5003](https://github.com/prefix-dev/pixi/issues/5003).
//! A "member" is a subdirectory that contains its own pixi-compatible manifest
//! (`pixi.toml`, `pyproject.toml`, or `mojoproject.toml`) with a
//! `[package].name` declaration. Members form a tree: when a member contains
//! another member under it, the inner one becomes a child in the tree.
//!
//! Discovery is purely structural — it does not parse the full manifest (which
//! may depend on workspace inheritance), it only peeks at the TOML pointers
//! needed to extract the package name and detect nested `[workspace]` blocks
//! (which are not supported).

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use indexmap::IndexMap;
use miette::{Diagnostic, NamedSource};
use pixi_consts::consts;
use thiserror::Error;

use crate::{
    ManifestKind, ManifestProvenance, ProvenanceError, TomlError, utils::WithSourceCode,
};

/// A tree of member packages rooted under a workspace directory.
#[derive(Debug, Default, Clone)]
pub struct MemberTree {
    members: IndexMap<String, MemberNode>,
}

/// A single member package node in the tree.
#[derive(Debug, Clone)]
pub struct MemberNode {
    /// The `[package].name` (or `[tool.pixi.package].name`) declared in the
    /// member manifest.
    pub name: String,
    /// Absolute path to the manifest file for this member.
    pub manifest_path: PathBuf,
    /// Absolute path to the directory containing the manifest.
    pub dir: PathBuf,
    /// Discriminates between pixi.toml / pyproject.toml / mojoproject.toml.
    pub kind: ManifestKind,
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
    /// depth-first, insertion order. `path_segments` is the chain of member
    /// names from the root (e.g. `vec!["a", "c"]` for a member addressable as
    /// `a::c`).
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
        "duplicate member package name `{name}`: found at `{first}` and `{second}`"
    )]
    DuplicateSibling {
        name: String,
        first: PathBuf,
        second: PathBuf,
    },

    #[error(
        "nested workspace is not supported under `hierarchical-tasks`: `{dir}` declares its own `[workspace]`"
    )]
    NestedWorkspace { dir: PathBuf },
}

/// Discovers nested member packages rooted at `workspace_dir`.
///
/// Walks the directory tree under `workspace_dir`, skipping common build /
/// cache / VCS directories. When a directory contains a pixi-compatible
/// manifest with a `[package].name`, that directory becomes a member and
/// descent continues inside it for further nested members.
///
/// Returns an empty tree if no members are found. The root manifest at
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
    // Deterministic order.
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| should_descend(p))
        .collect();
    entries.sort();

    for entry in entries {
        let Some(provenance) = provenance_from_dir(&entry) else {
            // Not a manifest-carrying dir itself, but members may exist deeper.
            walk(&entry, members)?;
            continue;
        };

        // Peek at the TOML. We do NOT run the full manifest deserializer here
        // because sub-members may rely on workspace inheritance that isn't
        // resolved at this layer. We only need the package name and a check
        // for a nested `[workspace]`.
        let contents = provenance
            .read()?
            .map(Arc::<str>::from);
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

        let (has_workspace, name) = match provenance.kind {
            ManifestKind::Pixi | ManifestKind::MojoProject => (
                toml.pointer("/workspace").is_some()
                    || toml.pointer("/project").is_some(),
                toml.pointer("/package/name")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
            ),
            ManifestKind::Pyproject => (
                toml.pointer("/tool/pixi/workspace").is_some()
                    || toml.pointer("/tool/pixi/project").is_some(),
                toml.pointer("/tool/pixi/package/name")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .or_else(|| {
                        toml.pointer("/project/name")
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                    }),
            ),
        };

        if has_workspace {
            return Err(MemberDiscoveryError::NestedWorkspace { dir: entry.clone() });
        }

        let Some(name) = name else {
            // No package name = not a member. Keep descending.
            walk(&entry, members)?;
            continue;
        };

        // Recurse to find nested members under this one.
        let mut children = IndexMap::new();
        walk(&entry, &mut children)?;

        let node = MemberNode {
            name: name.clone(),
            manifest_path: provenance.path.clone(),
            dir: entry.clone(),
            kind: provenance.kind,
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

    fn pkg_toml(name: &str) -> String {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n")
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
        write(&tmp.path().join("a/pixi.toml"), &pkg_toml("a"));
        write(&tmp.path().join("b/pixi.toml"), &pkg_toml("b"));

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
        write(&tmp.path().join("a/pixi.toml"), &pkg_toml("a"));
        write(&tmp.path().join("a/c/pixi.toml"), &pkg_toml("c"));

        let tree = discover_members(tmp.path()).unwrap();
        let a = tree.resolve(["a"]).expect("a should exist");
        assert!(a.children.contains_key("c"));
        assert!(tree.resolve(["a", "c"]).is_some());
    }

    #[test]
    fn intermediate_dir_without_manifest_is_transparent() {
        // b/ has no manifest; b/c/pixi.toml becomes a top-level member `c`.
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("b/c/pixi.toml"), &pkg_toml("c"));

        let tree = discover_members(tmp.path()).unwrap();
        assert!(tree.resolve(["c"]).is_some());
        assert!(tree.resolve(["b"]).is_none());
    }

    #[test]
    fn skips_common_build_and_hidden_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("target/pixi.toml"), &pkg_toml("should_not_appear"));
        write(&tmp.path().join(".pixi/pixi.toml"), &pkg_toml("should_not_appear_either"));
        write(&tmp.path().join("node_modules/pixi.toml"), &pkg_toml("also_nope"));
        write(&tmp.path().join("ok/pixi.toml"), &pkg_toml("ok"));

        let tree = discover_members(tmp.path()).unwrap();
        let names: Vec<_> = tree.members().keys().cloned().collect();
        assert_eq!(names, vec!["ok".to_string()]);
    }

    #[test]
    fn manifest_without_package_name_is_not_a_member_but_is_transparent() {
        // A pixi.toml without [package] is ignored as a member, but we still
        // descend beneath it to find deeper members.
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("tools/pixi.toml"),
            "[workspace]\nchannels=[]\nplatforms=[]\n",
        );
        // Actually [workspace] triggers NestedWorkspace; use a bare file instead.
        // Overwrite with something that has neither [workspace] nor [package].
        write(&tmp.path().join("tools/pixi.toml"), "# empty\n");
        write(&tmp.path().join("tools/inner/pixi.toml"), &pkg_toml("inner"));

        let tree = discover_members(tmp.path()).unwrap();
        // `tools` is not a member, but `inner` is found beneath it.
        assert!(tree.resolve(["inner"]).is_some());
        assert!(tree.resolve(["tools"]).is_none());
    }

    #[test]
    fn duplicate_sibling_name_errors() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a/pixi.toml"), &pkg_toml("same"));
        write(&tmp.path().join("b/pixi.toml"), &pkg_toml("same"));

        let err = discover_members(tmp.path()).unwrap_err();
        assert!(
            matches!(err, MemberDiscoveryError::DuplicateSibling { ref name, .. } if name == "same")
        );
    }

    #[test]
    fn nested_workspace_errors() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("sub/pixi.toml"),
            "[workspace]\nchannels=[]\nplatforms=[]\n",
        );
        let err = discover_members(tmp.path()).unwrap_err();
        assert!(matches!(err, MemberDiscoveryError::NestedWorkspace { .. }));
    }

    #[test]
    fn deeply_nested_transparent_chain() {
        // root/a(member)/mid(no manifest)/c(member)
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a/pixi.toml"), &pkg_toml("a"));
        write(&tmp.path().join("a/mid/c/pixi.toml"), &pkg_toml("c"));

        let tree = discover_members(tmp.path()).unwrap();
        assert!(tree.resolve(["a", "c"]).is_some());
    }

    #[test]
    fn walk_yields_paths_in_insertion_order() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a/pixi.toml"), &pkg_toml("a"));
        write(&tmp.path().join("a/c/pixi.toml"), &pkg_toml("c"));
        write(&tmp.path().join("b/pixi.toml"), &pkg_toml("b"));

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
}
