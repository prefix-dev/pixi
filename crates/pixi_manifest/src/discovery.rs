//! This module provides the [`WorkspaceDiscoverer`] struct which can be used to
//! discover the workspace manifest in a directory tree.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use miette::{Diagnostic, NamedSource};
use pixi_consts::consts;
use thiserror::Error;
use toml_span::Deserialize;

use crate::toml::ExternalWorkspaceProperties;
use crate::{
    pyproject::PyProjectManifest,
    toml::{ExternalPackageProperties, TomlManifest},
    utils::WithSourceCode,
    warning::WarningWithSource,
    AssociateProvenance, ManifestKind, ManifestProvenance, ManifestSource, PackageManifest,
    ProvenanceError, TomlError, WithProvenance, WithWarnings, WorkspaceManifest,
};

/// A helper struct to discover the workspace manifest in a directory tree from
/// a given path. The discoverer will walk up the directory tree until it finds
/// a workspace.
///
/// It can also collect the first package that was found on the way to the
/// workspace. See [`WorkspaceDiscoverer::with_closest_package`] for more
/// information.
pub struct WorkspaceDiscoverer {
    /// The current path
    start: DiscoveryStart,

    /// Also discover the package closest to the current directory.
    discover_package: bool,
}

/// A workspace discovered by calling [`WorkspaceDiscoverer::discover`].
#[derive(Debug)]
pub struct Manifests {
    /// The discovered workspace manifest
    pub workspace: WithProvenance<WorkspaceManifest>,

    /// If requested, contains the package manifest for the closest package in
    /// the workspace. `None` if there is no package manifest on the path to the
    /// workspace.
    /// If not requested this might still contain the package manifest stored in
    /// the same manifest as the workspace.
    pub package: Option<WithProvenance<PackageManifest>>,
}

/// An error that may occur when loading a discovered workspace directly from a
/// file.
#[derive(Debug, Error, Diagnostic)]
pub enum LoadManifestsError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(#[from] Box<WithSourceCode<TomlError, NamedSource<Arc<str>>>>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    ProvenanceError(#[from] ProvenanceError),
}

impl Manifests {
    /// Constructs a new instance from a specific workspace manifest.
    pub fn from_workspace_manifest_path(
        workspace_manifest_path: PathBuf,
    ) -> Result<WithWarnings<Self, WarningWithSource>, LoadManifestsError> {
        let provenance = ManifestProvenance::from_path(workspace_manifest_path)?;
        let contents = provenance.read()?;
        Self::from_workspace_source(contents.into_inner().with_provenance(provenance))
    }

    /// Constructs a new instance from a specific workspace manifest that in
    /// memory.
    pub fn from_workspace_source<S: AsRef<str>>(
        WithProvenance {
            value: source,
            provenance,
        }: WithProvenance<S>,
    ) -> Result<WithWarnings<Self, WarningWithSource>, LoadManifestsError> {
        let build_source_code = || {
            NamedSource::new(
                provenance.path.to_string_lossy(),
                Arc::from(source.as_ref()),
            )
            .with_language(provenance.kind.language())
        };

        // Parse the TOML from the manifest.
        let mut toml = match toml_span::parse(source.as_ref()) {
            Ok(toml) => toml,
            Err(e) => {
                return Err(Box::new(WithSourceCode {
                    error: TomlError::from(e),
                    source: build_source_code(),
                })
                .into())
            }
        };

        // Parse the manifest as a workspace based on the type of manifest.
        let manifest_dir = provenance.path.parent().expect("a file must have a parent");
        let parsed_manifests = match provenance.kind {
            ManifestKind::Pixi => TomlManifest::deserialize(&mut toml)
                .map_err(TomlError::from)
                .and_then(|manifest| {
                    manifest.into_workspace_manifest(
                        ExternalWorkspaceProperties::default(),
                        Some(manifest_dir),
                    )
                }),
            ManifestKind::Pyproject => PyProjectManifest::deserialize(&mut toml)
                .map_err(TomlError::from)
                .and_then(|manifest| manifest.into_workspace_manifest(Some(manifest_dir))),
        };

        // Handle any errors that occurred during parsing.
        let (workspace_manifest, package_manifest, warnings) = match parsed_manifests {
            Ok(parsed_manifests) => parsed_manifests,
            Err(toml_error) => {
                return Err(Box::new(WithSourceCode {
                    error: toml_error,
                    source: build_source_code(),
                })
                .into())
            }
        };

        // Associate the warnings with the source code.
        let warnings = if warnings.is_empty() {
            vec![]
        } else {
            let source = build_source_code();
            warnings
                .into_iter()
                .map(|warning| WithSourceCode {
                    error: warning,
                    source: source.clone(),
                })
                .collect()
        };

        Ok(WithWarnings::from(Self {
            package: package_manifest
                .map(|package_manifest| WithProvenance::new(package_manifest, provenance.clone())),
            workspace: WithProvenance {
                provenance,
                value: workspace_manifest,
            },
        })
        .with_warnings(warnings))
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum ExplicitManifestError {
    #[error("could not find '{}'", .0.display())]
    MissingManifest(PathBuf),

    #[error(transparent)]
    InvalidManifest(ProvenanceError),
}

#[derive(Debug, Error, Diagnostic)]
pub enum WorkspaceDiscoveryError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(#[from] Box<WithSourceCode<TomlError, NamedSource<Arc<str>>>>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    ExplicitManifestError(#[from] ExplicitManifestError),

    #[error("cannot canonicalize path '{1}' while searching for a manifest.")]
    Canonicalize(#[source] std::io::Error, PathBuf),
}

#[allow(clippy::large_enum_variant)]
enum EitherManifest {
    Pixi(TomlManifest),
    Pyproject(PyProjectManifest),
}

/// Defines where the search for the workspace should start.
#[derive(Debug, Clone)]
pub enum DiscoveryStart {
    /// Start the search from the given directory.
    ///
    /// This will search for a workspace manifest in the given directory and its
    /// parent directories.
    SearchRoot(PathBuf),

    /// Use the manifest file at the given path. Only search for a workspace if
    /// the specified manifest is not a workspace.
    ///
    /// This differs from specifying the parent directory of the manifest file
    /// in that it is also possible to specify a manifest that is not the
    /// default preferred format (e.g. `pyproject.toml`).
    ExplicitManifest(PathBuf),
}

impl DiscoveryStart {
    /// Returns the path of the directory or file to start the search from.
    pub fn root(&self) -> &Path {
        match self {
            DiscoveryStart::SearchRoot(root) => root.as_path(),
            DiscoveryStart::ExplicitManifest(manifest) => manifest.as_path(),
        }
    }
}

impl WorkspaceDiscoverer {
    /// Constructs a new instance from the current path.
    pub fn new(start: DiscoveryStart) -> Self {
        Self {
            start,
            discover_package: false,
        }
    }

    /// Also discover the package closest to the current directory.
    ///
    /// If set to `true`, the discoverer will also try to find the closest
    /// package manifest on the way to the workspace. Or if the workspace
    /// manifest also contains a package manifest it will be used as the closest
    /// package manifest.
    pub fn with_closest_package(self, discover_package: bool) -> Self {
        Self {
            discover_package,
            ..self
        }
    }

    /// Discover the workspace manifest in the directory tree.
    pub fn discover(
        self,
    ) -> Result<Option<WithWarnings<Manifests, WarningWithSource>>, WorkspaceDiscoveryError> {
        #[derive(Clone)]
        enum SearchPath {
            Explicit(PathBuf),
            Directory(PathBuf),
        }

        // Walk up the directory tree until we find a workspace manifest.
        let mut warnings = Vec::new();
        let mut closest_package_manifest = None;
        let mut next_search_path = match &self.start {
            DiscoveryStart::SearchRoot(root) => Some(SearchPath::Directory(
                dunce::canonicalize(root)
                    .map_err(|e| WorkspaceDiscoveryError::Canonicalize(e, root.clone()))?,
            )),
            DiscoveryStart::ExplicitManifest(manifest_path) => Some(SearchPath::Explicit(
                dunce::canonicalize(manifest_path)
                    .map_err(|e| WorkspaceDiscoveryError::Canonicalize(e, manifest_path.clone()))?,
            )),
        };
        while let Some(search_path) = next_search_path {
            let (next, provenance) = match search_path {
                SearchPath::Explicit(ref explicit) => {
                    if !explicit.exists() {
                        return Err(
                            ExplicitManifestError::MissingManifest(explicit.to_path_buf()).into(),
                        );
                    }
                    if explicit.is_file() {
                        let provenance = ManifestProvenance::from_path(explicit.clone())
                            .map_err(ExplicitManifestError::InvalidManifest)?;
                        let next_dir = explicit
                            .parent()
                            .expect("the manifest itself must have a parent directory")
                            .parent();
                        (next_dir.map(ToOwned::to_owned), Some(provenance))
                    } else {
                        let provenance = Self::provenance_from_dir(explicit).ok_or(
                            ExplicitManifestError::InvalidManifest(
                                ProvenanceError::UnrecognizedManifestFormat,
                            ),
                        )?;
                        tracing::info!(
                            "Found manifest in directory: {:?}, continuing further.",
                            provenance.path
                        );
                        (explicit.parent().map(ToOwned::to_owned), Some(provenance))
                    }
                }
                SearchPath::Directory(ref manifest_dir_path) => {
                    // Check if a pixi.toml file exists in the current directory.
                    let provenance = Self::provenance_from_dir(manifest_dir_path);
                    if provenance.is_some() {
                        tracing::info!(
                            "Found manifest in directory: {:?}, continuing further.",
                            manifest_dir_path
                        );
                    }
                    (
                        manifest_dir_path.parent().map(ToOwned::to_owned),
                        provenance,
                    )
                }
            };

            next_search_path = next.map(SearchPath::Directory);

            let Some(provenance) = provenance else {
                // If there is no manifest for the current search path, continue searching.
                continue;
            };

            // Read the contents of the manifest file.
            let contents = provenance.read()?.map(Arc::<str>::from);

            // Cheap check to see if the manifest contains a pixi section.
            if let ManifestSource::PyProjectToml(source) = &contents {
                if !source.contains("[tool.pixi")
                    && !matches!(search_path.clone(), SearchPath::Explicit(_))
                {
                    continue;
                }
            }

            let source = contents.into_named(provenance.absolute_path().to_string_lossy());

            // Parse the TOML from the manifest
            let mut toml = match toml_span::parse(source.inner()) {
                Ok(toml) => toml,
                Err(e) => {
                    return Err(Box::new(WithSourceCode {
                        error: TomlError::from(e),
                        source,
                    })
                    .into())
                }
            };

            // Parse the workspace manifest.
            let manifest_dir = provenance.path.parent().expect("a file must have a parent");
            let parsed_manifest = match provenance.kind {
                ManifestKind::Pixi => {
                    if closest_package_manifest.is_some() && toml.pointer("/workspace").is_none() {
                        // The manifest does not contain a workspace section, and we don't care
                        // about the package section.
                        continue;
                    }

                    // Parse as a pixi.toml manifest
                    let manifest = match TomlManifest::deserialize(&mut toml) {
                        Ok(manifest) => manifest,
                        Err(err) => {
                            return Err(Box::new(WithSourceCode {
                                error: TomlError::from(err),
                                source,
                            })
                            .into())
                        }
                    };

                    if manifest.has_workspace() {
                        // Parse the manifest as a workspace manifest if it contains a workspace
                        manifest.into_workspace_manifest(
                            ExternalWorkspaceProperties::default(),
                            Some(manifest_dir),
                        )
                    } else {
                        if self.discover_package {
                            // Otherwise store the manifest for later to parse as the closest
                            // package manifest.
                            closest_package_manifest = closest_package_manifest.or(Some((
                                EitherManifest::Pixi(manifest),
                                source,
                                provenance,
                            )));
                        }
                        continue;
                    }
                }
                ManifestKind::Pyproject => {
                    if closest_package_manifest.is_some()
                        && toml.pointer("/tool/pixi/workspace").is_none()
                    {
                        // The manifest does not contain a workspace section, and we don't care
                        // about the package section.
                        continue;
                    }

                    let manifest = match PyProjectManifest::deserialize(&mut toml) {
                        Ok(manifest) => manifest,
                        Err(err) => {
                            return Err(Box::new(WithSourceCode {
                                error: TomlError::from(err),
                                source,
                            })
                            .into())
                        }
                    };

                    if manifest.has_pixi_workspace() {
                        // Parse the manifest as a workspace manifest if it
                        // contains a workspace
                        manifest.into_workspace_manifest(Some(manifest_dir))
                    } else {
                        if self.discover_package {
                            // Otherwise store the manifest for later to parse as the closest
                            // package manifest.
                            closest_package_manifest = closest_package_manifest.or(Some((
                                EitherManifest::Pyproject(manifest),
                                source,
                                provenance,
                            )));
                        }
                        continue;
                    }
                }
            };

            let (workspace_manifest, package_manifest, workspace_warnings) = match parsed_manifest {
                Ok(parsed_manifest) => parsed_manifest,
                Err(error) => return Err(Box::new(WithSourceCode { error, source }).into()),
            };

            // Add the errors from the workspace manifest to the list of warnings.
            warnings.extend(
                workspace_warnings
                    .into_iter()
                    .map(|warning| WithSourceCode {
                        error: warning,
                        source: source.clone(),
                    }),
            );

            // Incorporate the workspace information into the package manifest.
            let closest_package_manifest = match closest_package_manifest {
                None => {
                    // If no package manifest is found on the way to the workspace manifest, we can
                    // use the package defined in the same manifest as the workspace itself.
                    package_manifest.map(|package_manifest| {
                        WithProvenance::new(package_manifest, provenance.clone())
                    })
                }
                Some((package_manifest, source, provenance)) => {
                    // Convert a found manifest into a package manifest using the workspace
                    // manifest.
                    let manifest_dir = provenance.path.parent().expect("a file must have a parent");
                    let package_manifest = match package_manifest {
                        EitherManifest::Pixi(manifest) => manifest.into_package_manifest(
                            ExternalPackageProperties::default(),
                            &workspace_manifest,
                            Some(manifest_dir),
                        ),
                        EitherManifest::Pyproject(manifest) => {
                            manifest.into_package_manifest(&workspace_manifest, Some(manifest_dir))
                        }
                    };

                    match package_manifest {
                        Ok((package_manifest, package_warnings)) => {
                            warnings.extend(package_warnings.into_iter().map(|warning| {
                                WithSourceCode {
                                    error: warning,
                                    source: source.clone(),
                                }
                            }));
                            Some(WithProvenance::new(package_manifest, provenance))
                        }
                        Err(error) => {
                            return Err(Box::new(WithSourceCode { error, source }).into());
                        }
                    }
                }
            };

            return Ok(Some(
                WithWarnings::from(Manifests {
                    workspace: WithProvenance::new(workspace_manifest, provenance),
                    package: closest_package_manifest,
                })
                .with_warnings(warnings),
            ));
        }

        Ok(None)
    }

    /// Discover the workspace manifest in a directory.
    fn provenance_from_dir(dir: &Path) -> Option<ManifestProvenance> {
        let pixi_toml_path = dir.join(consts::PROJECT_MANIFEST);
        let pyproject_toml_path = dir.join(consts::PYPROJECT_MANIFEST);
        if pixi_toml_path.is_file() {
            Some(ManifestProvenance::new(pixi_toml_path, ManifestKind::Pixi))
        } else if pyproject_toml_path.is_file() {
            Some(ManifestProvenance::new(
                pyproject_toml_path,
                ManifestKind::Pyproject,
            ))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod test {
    use std::{fmt::Write, path::Path};

    use rstest::*;

    use super::*;
    use crate::utils::test_utils::format_diagnostic;

    #[rstest]
    #[case::root("")]
    #[case::empty("empty")]
    #[case::package_a("package_a")]
    #[case::package_b("package_a/package_b")]
    #[case::nested_workspace("nested-workspace")]
    #[case::nested_pyproject_workspace("nested-pyproject-workspace")]
    #[case::nested_pixi_project_in_nested_pyproject_workspace(
        "nested-pyproject-workspace/nested-pixi-project"
    )]
    #[case::nested_pyproject_in_nested_pyproject_workspace(
        "nested-pyproject-workspace/nested-pyproject"
    )]
    #[case::nested_non_pixi_pyproject_in_nested_pyproject_workspace(
        "nested-pyproject-workspace/nested-non-pixi-pyproject"
    )]
    #[case::non_pixi_build("non-pixi-build")]
    #[case::non_pixi_build_project("non-pixi-build/project")]
    fn test_workspace_discoverer(#[case] subdir: &str) {
        let test_data_root = dunce::canonicalize(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/workspace-discovery"),
        )
        .unwrap();

        let snapshot =
            match WorkspaceDiscoverer::new(DiscoveryStart::SearchRoot(test_data_root.join(subdir)))
                .with_closest_package(true)
                .discover()
            {
                Ok(None) => "Not found!".to_owned(),
                Ok(Some(WithWarnings {
                    value: discovered, ..
                })) => {
                    let rel_path =
                        pathdiff::diff_paths(&discovered.workspace.provenance.path, test_data_root)
                            .unwrap_or(discovered.workspace.provenance.path);
                    let mut snapshot = String::new();
                    writeln!(
                        &mut snapshot,
                        "Discovered workspace at: {}\n- Name: {}",
                        rel_path.display().to_string().replace("\\", "/"),
                        &discovered.workspace.value.workspace.name
                    )
                    .unwrap();

                    if let Some(package) = &discovered.package {
                        writeln!(
                            &mut snapshot,
                            "Package: {} @ {}",
                            &package.value.package.name, &package.value.package.version,
                        )
                        .unwrap();
                    }

                    snapshot
                }
                Err(e) => format_diagnostic(&e),
            };

        insta::with_settings!({
            snapshot_suffix => subdir.replace("/", "_"),
        }, {
            insta::assert_snapshot!(snapshot);
        });
    }

    #[rstest]
    #[case::root("")]
    #[case::pixi("pixi.toml")]
    #[case::empty("empty")]
    #[case::package_specific("package_a/pixi.toml")]
    fn test_explicit_workspace_discoverer(#[case] subdir: &str) {
        let test_data_root = dunce::canonicalize(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/workspace-discovery"),
        )
        .unwrap();

        let snapshot = match WorkspaceDiscoverer::new(DiscoveryStart::ExplicitManifest(
            test_data_root.join(subdir),
        ))
        .with_closest_package(true)
        .discover()
        {
            Ok(None) => "Not found!".to_owned(),
            Ok(Some(WithWarnings {
                value: discovered, ..
            })) => {
                let rel_path =
                    pathdiff::diff_paths(&discovered.workspace.provenance.path, test_data_root)
                        .unwrap_or(discovered.workspace.provenance.path);

                let mut snapshot = String::new();
                writeln!(
                    &mut snapshot,
                    "Discovered workspace at: {}\n- Name: {}",
                    rel_path.display().to_string().replace("\\", "/"),
                    &discovered.workspace.value.workspace.name
                )
                .unwrap();

                if let Some(package) = &discovered.package {
                    writeln!(
                        &mut snapshot,
                        "Package: {} @ {}",
                        &package.value.package.name, &package.value.package.version,
                    )
                    .unwrap();
                }

                snapshot
            }
            Err(e) => format_diagnostic(&e),
        };

        insta::with_settings!({
            snapshot_suffix => subdir.replace("/", "_"),
        }, {
            insta::assert_snapshot!(snapshot);
        });
    }

    #[test]
    fn test_non_existing_discovery() {
        // Split from the previous rstests, to avoid insta snapshot path conflicts in the error.
        let test_data_root = dunce::canonicalize(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/workspace-discovery"),
        )
        .unwrap();

        let err = WorkspaceDiscoverer::new(DiscoveryStart::SearchRoot(
            test_data_root.join("non-existing"),
        ))
        .with_closest_package(true)
        .discover()
        .expect_err("Expected an error");

        assert!(matches!(err, WorkspaceDiscoveryError::Canonicalize(_, _)));
    }
}
