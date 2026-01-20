use crate::{GitSpec, PathSourceSpec, SourceLocationSpec, SourceSpec, Subdirectory, UrlSourceSpec};
use pixi_consts::consts::{KNOWN_MANIFEST_FILES, RATTLER_BUILD_FILE_NAMES, ROS_BACKEND_FILE_NAMES};
use pixi_path::normalize;
use typed_path::Utf8TypedPath;

/// `SourceAnchor` represents the resolved base location of a `SourceSpec`.
/// It serves as a reference point for interpreting relative or recursive
/// source specifications, enabling consistent resolution of nested sources.
#[derive(Clone, Debug)]
pub enum SourceAnchor {
    /// The source is relative to the workspace root.
    Workspace,

    /// The source is relative to another source package.
    Source(SourceLocationSpec),
}

impl From<SourceSpec> for SourceAnchor {
    fn from(value: SourceSpec) -> Self {
        SourceAnchor::Source(value.location)
    }
}

impl From<SourceLocationSpec> for SourceAnchor {
    fn from(value: SourceLocationSpec) -> Self {
        SourceAnchor::Source(value)
    }
}

impl SourceAnchor {
    /// Resolve a source location spec relative to this anchor.
    pub fn resolve(&self, spec: SourceLocationSpec) -> SourceLocationSpec {
        // If this instance is already anchored to the workspace we can simply return
        // immediately.
        let SourceAnchor::Source(base) = self else {
            return match spec {
                SourceLocationSpec::Url(url) => SourceLocationSpec::Url(url),
                SourceLocationSpec::Git(git) => SourceLocationSpec::Git(git),
                SourceLocationSpec::Path(PathSourceSpec { path }) => {
                    SourceLocationSpec::Path(PathSourceSpec {
                        // Normalize the input path.
                        path: normalize::normalize_typed(path.to_path()),
                    })
                }
            };
        };

        // Only path specs can be relative.
        let SourceLocationSpec::Path(PathSourceSpec { path }) = spec else {
            return spec;
        };

        // If the path is absolute we can just return it.
        if path.is_absolute() || path.starts_with("~") {
            return SourceLocationSpec::Path(PathSourceSpec { path });
        }

        match base {
            SourceLocationSpec::Path(PathSourceSpec { path: base }) => {
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
                SourceLocationSpec::Path(PathSourceSpec {
                    path: relative_path,
                })
            }
            SourceLocationSpec::Url(UrlSourceSpec { .. }) => {
                unimplemented!("Cannot resolve relative paths for URL sources")
            }
            SourceLocationSpec::Git(GitSpec {
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
                SourceLocationSpec::Git(GitSpec {
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
        .map(|name| {
            KNOWN_MANIFEST_FILES.contains(&name)
                || RATTLER_BUILD_FILE_NAMES.contains(&name)
                || ROS_BACKEND_FILE_NAMES.contains(&name)
        })
        .unwrap_or(false)
}
