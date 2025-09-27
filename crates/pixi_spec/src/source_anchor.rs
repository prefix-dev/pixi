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
    Source(SourceSpec),
}

impl From<SourceSpec> for SourceAnchor {
    fn from(value: SourceSpec) -> Self {
        SourceAnchor::Source(value)
    }
}

impl SourceAnchor {
    /// Resolve a source spec relative to this anchor.
    pub fn resolve(&self, spec: SourceSpec) -> SourceSpec {
        // If this instance is already anchored to the workspace we can simply return
        // immediately.
        let SourceAnchor::Source(base) = self else {
            return match spec.location {
                SourceLocationSpec::Url(url) => SourceSpec {
                    location: SourceLocationSpec::Url(url),
                },
                SourceLocationSpec::Git(git) => SourceSpec {
                    location: SourceLocationSpec::Git(git),
                },
                SourceLocationSpec::Path(PathSourceSpec { path }) => {
                    SourceSpec {
                        location: SourceLocationSpec::Path(PathSourceSpec {
                            // Normalize the input path.
                            path: normalize_typed(path.to_path()),
                        }),
                    }
                }
            };
        };

        // Only path specs can be relative.
        let SourceSpec {
            location: SourceLocationSpec::Path(PathSourceSpec { path }),
        } = spec
        else {
            return spec;
        };

        // If the path is absolute we can just return it.
        if path.is_absolute() || path.starts_with("~") {
            return SourceSpec {
                location: SourceLocationSpec::Path(PathSourceSpec { path }),
            };
        }

        match &base.location {
            SourceLocationSpec::Path(PathSourceSpec { path: base }) => {
                let relative_path = normalize_typed(base.join(path).to_path());
                SourceSpec {
                    location: SourceLocationSpec::Path(PathSourceSpec {
                        path: relative_path,
                    }),
                }
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
                SourceSpec {
                    location: SourceLocationSpec::Git(GitSpec {
                        git: git.clone(),
                        rev: rev.clone(),
                        subdirectory: Some(relative_subdir.to_string()),
                    }),
                }
            }
        }
    }
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
