use pixi_consts::consts::KNOWN_MANIFEST_FILES;
use typed_path::{
    Utf8Component, Utf8Encoding, Utf8Path, Utf8PathBuf, Utf8TypedPath, Utf8TypedPathBuf,
};

use crate::{GitSpec, PathSourceSpec, SourceLocationSpec, SourceSpec, UrlSourceSpec};

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
                        path: normalize_typed(path.to_path()),
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

                let relative_path = normalize_typed(base_dir.join(path).to_path());
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
                let relative_subdir = normalize_typed(
                    Utf8TypedPath::from(subdirectory.as_deref().unwrap_or_default())
                        .join(path)
                        .to_path(),
                );
                SourceLocationSpec::Git(GitSpec {
                    git: git.clone(),
                    rev: rev.clone(),
                    subdirectory: Some(relative_subdir.to_string()),
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

/// A slightly modified version of [`Utf8TypedPath::normalize`] that retains
/// `..` components that lead outside the path.
fn normalize_typed(path: Utf8TypedPath<'_>) -> Utf8TypedPathBuf {
    match path {
        Utf8TypedPath::Unix(path) => Utf8TypedPathBuf::Unix(normalize(path)),
        Utf8TypedPath::Windows(path) => Utf8TypedPathBuf::Windows(normalize(path)),
    }
}

/// A slightly modified version of [`Utf8Path::normalize`] that retains `..`
/// components that lead outside the path.
fn normalize<T: Utf8Encoding>(path: &Utf8Path<T>) -> Utf8PathBuf<T> {
    let mut components = Vec::new();
    for component in path.components() {
        if !component.is_current() && !component.is_parent() {
            components.push(component);
        } else if component.is_parent() {
            if let Some(last) = components.last() {
                if last.is_normal() {
                    components.pop();
                } else {
                    components.push(component);
                }
            } else {
                components.push(component);
            }
        }
    }

    let mut path = Utf8PathBuf::<T>::new();

    for component in components {
        path.push(component.as_str());
    }

    path
}
