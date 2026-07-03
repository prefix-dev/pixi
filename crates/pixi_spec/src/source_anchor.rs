use crate::{GitLocationSpec, PathSpec, SourceLocationSpec, SourceSpec, Subdirectory};
use pixi_consts::consts::KNOWN_MANIFEST_FILES;
use pixi_path::normalize;
use typed_path::Utf8TypedPath;

/// `SourceAnchor` represents the resolved base location of a source spec.
/// It serves as a reference point for interpreting relative or recursive
/// source specifications, enabling consistent resolution of nested sources.
///
/// Only the location matters for anchoring, so this holds a
/// [`SourceLocationSpec`] rather than a full [`SourceSpec`].
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum SourceAnchor {
    /// The source is relative to the workspace root.
    Workspace,

    /// The source is relative to another source package.
    Source(SourceLocationSpec),
}

impl From<SourceLocationSpec> for SourceAnchor {
    fn from(value: SourceLocationSpec) -> Self {
        SourceAnchor::Source(value)
    }
}

impl From<SourceSpec> for SourceAnchor {
    fn from(value: SourceSpec) -> Self {
        SourceAnchor::Source(value.location)
    }
}

impl SourceAnchor {
    /// Resolve a source spec relative to this anchor. Only the location is
    /// resolved; the spec's matchspec selectors ride along unchanged, since
    /// the base's selectors apply to the base, not to its nested children.
    pub fn resolve(&self, spec: SourceSpec) -> SourceSpec {
        let SourceSpec {
            location,
            matchspec,
        } = spec;
        let location = self.resolve_location(location);
        SourceSpec {
            location,
            matchspec,
        }
    }

    /// Resolve a source location relative to this anchor.
    pub fn resolve_location(&self, location: SourceLocationSpec) -> SourceLocationSpec {
        // If this instance is already anchored to the workspace we can simply return
        // immediately.
        let SourceAnchor::Source(base) = self else {
            return match location {
                SourceLocationSpec::Url(url) => SourceLocationSpec::Url(url),
                SourceLocationSpec::Git(git) => SourceLocationSpec::Git(git),
                SourceLocationSpec::Path(PathSpec { path }) => {
                    SourceLocationSpec::Path(PathSpec {
                        // Normalize the input path.
                        path: normalize::normalize_typed(path.to_path()),
                    })
                }
            };
        };

        // Only path specs can be relative.
        let SourceLocationSpec::Path(PathSpec { path }) = location else {
            return location;
        };

        // If the path is absolute we can just return it.
        if path.is_absolute() || path.starts_with("~") {
            return SourceLocationSpec::Path(PathSpec { path });
        }

        match base {
            SourceLocationSpec::Path(PathSpec { path: base }) => {
                // Use the parent directory as the base when the base path points to
                // a manifest file (e.g., `package-a/pixi.toml` -> `package-a`).
                // This ensures relative paths like `../package-b` resolve correctly.
                //
                // We check for known manifest file names rather than using filesystem
                // checks because the path may be relative to the workspace root,
                // not the current working directory.
                let base_dir = if is_known_manifest_file(base.to_path()) {
                    base.parent().unwrap_or_else(|| base.to_path())
                } else {
                    base.to_path()
                };

                let relative_path = normalize::normalize_typed(base_dir.join(path).to_path());
                SourceLocationSpec::Path(PathSpec {
                    path: relative_path,
                })
            }
            SourceLocationSpec::Url(_) => {
                unimplemented!("Cannot resolve relative paths for URL sources")
            }
            SourceLocationSpec::Git(GitLocationSpec {
                git,
                rev,
                subdirectory,
            }) => {
                let base_subdir = subdirectory.as_path().to_string_lossy();
                let relative_subdir = normalize::normalize_typed(
                    Utf8TypedPath::from(base_subdir.as_ref())
                        .join(path)
                        .to_path(),
                );
                // Convert to Subdirectory (defaults to empty if normalization results in empty)
                let subdir_str = relative_subdir.to_string();
                let subdirectory = if subdir_str.is_empty() {
                    Subdirectory::default()
                } else {
                    Subdirectory::try_from(subdir_str).unwrap_or_default()
                };
                SourceLocationSpec::Git(GitLocationSpec {
                    git: git.clone(),
                    rev: rev.clone(),
                    subdirectory,
                })
            }
        }
    }

    /// Inverse of [`SourceAnchor::resolve_location`]: express `target` (a
    /// location already resolved relative to the workspace root, as pinned
    /// source locations are) such that `self.resolve_location(result)`
    /// yields `target` again. Locations that resolve as identity (URL, git,
    /// absolute or `~`-prefixed paths) are returned unchanged. Returns
    /// `None` when the target cannot be expressed relative to this anchor.
    pub fn relativize_location(&self, target: SourceLocationSpec) -> Option<SourceLocationSpec> {
        let SourceLocationSpec::Path(PathSpec { path: target_path }) = &target else {
            // URL and git locations resolve as identity.
            return Some(target);
        };
        if target_path.to_path().is_absolute() || target_path.starts_with("~") {
            return Some(target);
        }
        let base = match self {
            // Workspace-anchored resolution only normalizes the path.
            SourceAnchor::Workspace => return Some(target),
            SourceAnchor::Source(SourceLocationSpec::Path(PathSpec { path })) => path.to_path(),
            // A relative path cannot be expressed against a git or URL base.
            SourceAnchor::Source(_) => return None,
        };
        let base_dir = if is_known_manifest_file(base) {
            base.parent().unwrap_or(base)
        } else {
            base
        };

        // Both sides are workspace-root-relative and normalized: peel the
        // common prefix, climb out of what remains of the base, then descend
        // into the remainder of the target.
        let base_components: Vec<&str> = base_dir.components().map(|c| c.as_str()).collect();
        let target_path = target_path.to_path();
        let target_components: Vec<&str> = target_path.components().map(|c| c.as_str()).collect();
        let common = base_components
            .iter()
            .zip(target_components.iter())
            .take_while(|(a, b)| a == b)
            .count();
        if base_components[common..].contains(&"..") {
            // Cannot climb out of an unknown ancestor.
            return None;
        }
        let mut parts: Vec<&str> = vec![".."; base_components.len() - common];
        parts.extend(&target_components[common..]);
        let rel = if parts.is_empty() {
            String::from(".")
        } else {
            parts.join("/")
        };
        Some(SourceLocationSpec::Path(PathSpec {
            path: typed_path::Utf8TypedPathBuf::from(rel),
        }))
    }
}

/// Checks if a path's filename matches a known manifest file name.
///
/// This is used to determine if a path points to a file rather than a directory,
/// without requiring filesystem access
/// which wouldn't work for paths relative
/// to the workspace root.
fn is_known_manifest_file(path: Utf8TypedPath<'_>) -> bool {
    path.file_name()
        .map(|name| KNOWN_MANIFEST_FILES.contains(&name))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_location(path: &str) -> SourceLocationSpec {
        SourceLocationSpec::Path(PathSpec { path: path.into() })
    }

    #[test]
    fn relativize_location_roundtrips_through_resolve() {
        let cases = [
            // (anchor, workspace-relative target, expected relative)
            ("package_a/pixi.toml", "package_b", "../package_b"),
            ("package_a", "package_b", "../package_b"),
            ("package_a", "package_a/nested", "nested"),
            ("package_a", "package_a", "."),
            ("nested/package_a", "libs/package_b", "../../libs/package_b"),
        ];
        for (anchor_path, target, expected) in cases {
            let anchor = SourceAnchor::from(path_location(anchor_path));
            let relativized = anchor
                .relativize_location(path_location(target))
                .unwrap_or_else(|| {
                    panic!("{target} must be expressible relative to {anchor_path}")
                });
            assert_eq!(
                relativized,
                path_location(expected),
                "relativize({anchor_path}, {target})"
            );
            assert_eq!(
                anchor.resolve_location(relativized),
                path_location(target),
                "resolve(relativize) must roundtrip for ({anchor_path}, {target})"
            );
        }
    }

    #[test]
    fn relativize_location_passes_identity_locations_through() {
        let anchor = SourceAnchor::from(path_location("package_a"));
        let absolute = path_location("/abs/package_b");
        assert_eq!(anchor.relativize_location(absolute.clone()), Some(absolute));

        let git = SourceLocationSpec::Git(GitLocationSpec {
            git: "https://github.com/example/repo".parse().unwrap(),
            rev: None,
            subdirectory: Subdirectory::default(),
        });
        assert_eq!(anchor.relativize_location(git.clone()), Some(git));

        // A relative path cannot be expressed against a git anchor.
        let git_anchor = SourceAnchor::from(SourceLocationSpec::Git(GitLocationSpec {
            git: "https://github.com/example/repo".parse().unwrap(),
            rev: None,
            subdirectory: Subdirectory::default(),
        }));
        assert_eq!(
            git_anchor.relativize_location(path_location("package_b")),
            None
        );
    }
}
