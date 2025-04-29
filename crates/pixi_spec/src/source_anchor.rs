use typed_path::Utf8TypedPath;

use crate::{GitSpec, PathSourceSpec, SourceSpec, UrlSourceSpec};

/// `SourceAnchor` represents the resolved base location of a `SourceSpec`.
/// It serves as a reference point for interpreting relative or recursive
/// source specifications, enabling consistent resolution of nested sources.
pub enum SourceAnchor {
    /// The source is relative to the workspace root.
    Workspace,

    /// The source is relative to another source package.
    Source(SourceSpec),
}

impl SourceAnchor {
    /// Resolve a source spec relative to this anchor.
    pub fn resolve(&self, spec: SourceSpec) -> SourceSpec {
        // If this instance is already anchored to the workspace we can simply return
        // immediately.
        let SourceAnchor::Source(base) = self else {
            return match spec {
                SourceSpec::Url(url) => SourceSpec::Url(url),
                SourceSpec::Git(git) => SourceSpec::Git(git),
                SourceSpec::Path(PathSourceSpec { path }) => SourceSpec::Path(PathSourceSpec {
                    // Normalize the input path.
                    path: path.normalize(),
                }),
            };
        };

        // Only path specs can be relative.
        let SourceSpec::Path(PathSourceSpec { path }) = spec else {
            return spec;
        };

        // If the path is absolute we can just return it.
        if path.is_absolute() || path.starts_with("~") {
            return SourceSpec::Path(PathSourceSpec { path });
        }

        match base {
            SourceSpec::Path(PathSourceSpec { path: base }) => {
                let relative_path = base.join(path).normalize();
                SourceSpec::Path(PathSourceSpec {
                    path: relative_path,
                })
            }
            SourceSpec::Url(UrlSourceSpec { .. }) => {
                unimplemented!("Cannot resolve relative paths for URL sources")
            }
            SourceSpec::Git(GitSpec {
                git,
                rev,
                subdirectory,
            }) => {
                let relative_subdir =
                    Utf8TypedPath::from(subdirectory.as_deref().unwrap_or_default())
                        .join(path)
                        .normalize();
                SourceSpec::Git(GitSpec {
                    git: git.clone(),
                    rev: rev.clone(),
                    subdirectory: Some(relative_subdir.to_string()),
                })
            }
        }
    }
}
