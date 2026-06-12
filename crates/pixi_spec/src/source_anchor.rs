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
