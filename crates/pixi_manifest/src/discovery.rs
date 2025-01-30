//! This module provides the [`WorkspaceDiscoverer`] struct which can be used to
//! discover the workspace manifest in a directory tree.

use std::{path::PathBuf, sync::Arc};

use miette::{Diagnostic, NamedSource};
use pixi_consts::consts;
use thiserror::Error;
use toml_span::Deserialize;

use crate::{
    pyproject::PyProjectManifest,
    toml::{ExternalPackageProperties, ExternalWorkspaceProperties, TomlManifest},
    utils::WithSourceCode,
    AssociateProvenance, ManifestKind, ManifestProvenance, ManifestSource, PackageManifest,
    ProvenanceError, TomlError, Warning, WithProvenance, WithWarnings, WorkspaceManifest,
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
    current_path: PathBuf,

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
pub enum DiscoveredWorkspaceError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(#[from] WithSourceCode<TomlError, NamedSource<Arc<str>>>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    ProvenanceError(#[from] ProvenanceError),
}

impl Manifests {
    /// Constructs a new instance from a specific workspace manifest.
    pub fn from_workspace_manifest_path(
        workspace_manifest_path: PathBuf,
    ) -> Result<
        WithWarnings<Self, WithSourceCode<Warning, NamedSource<Arc<str>>>>,
        DiscoveredWorkspaceError,
    > {
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
    ) -> Result<
        WithWarnings<Self, WithSourceCode<Warning, NamedSource<Arc<str>>>>,
        DiscoveredWorkspaceError,
    > {
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
                return Err(WithSourceCode {
                    error: TomlError::from(e),
                    source: build_source_code(),
                }
                .into())
            }
        };

        // Parse the manifest as a workspace based on the type of manifest.
        let parsed_manifests = match provenance.kind {
            ManifestKind::Pixi => TomlManifest::deserialize(&mut toml)
                .map_err(TomlError::from)
                .and_then(|manifest| {
                    manifest.into_workspace_manifest(ExternalWorkspaceProperties::default())
                }),
            ManifestKind::Pyproject => PyProjectManifest::deserialize(&mut toml)
                .map_err(TomlError::from)
                .and_then(|manifest| manifest.into_workspace_manifest()),
        };

        // Handle any errors that occurred during parsing.
        let (workspace_manifest, package_manifest, warnings) = match parsed_manifests {
            Ok(parsed_manifests) => parsed_manifests,
            Err(toml_error) => {
                return Err(WithSourceCode {
                    error: toml_error,
                    source: build_source_code(),
                }
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
pub enum WorkspaceDiscoveryError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(#[from] WithSourceCode<TomlError, NamedSource<Arc<str>>>),
}

enum EitherManifest {
    Pixi(TomlManifest),
    Pyproject(PyProjectManifest),
}

impl WorkspaceDiscoverer {
    /// Constructs a new instance from the current path.
    pub fn new(current_path: PathBuf) -> Self {
        Self {
            current_path,
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
    ) -> Result<
        Option<WithWarnings<Manifests, WithSourceCode<Warning, NamedSource<Arc<str>>>>>,
        WorkspaceDiscoveryError,
    > {
        // Walk up the directory tree until we find a workspace manifest.
        let mut warnings = Vec::new();
        let mut closest_package_manifest = None;
        let mut current_path = Some(self.current_path.as_path());
        while let Some(manifest_dir_path) = current_path {
            // Prepare the next directory to search.
            current_path = manifest_dir_path.parent();

            // Check if a pixi.toml file exists in the current directory.
            let pixi_toml_path = manifest_dir_path.join(consts::PROJECT_MANIFEST);
            let pyproject_toml_path = manifest_dir_path.join(consts::PYPROJECT_MANIFEST);
            let provenance = if pixi_toml_path.is_file() {
                ManifestProvenance::new(pixi_toml_path, ManifestKind::Pixi)
            } else if pyproject_toml_path.is_file() {
                ManifestProvenance::new(pyproject_toml_path, ManifestKind::Pyproject)
            } else {
                // Continue the search
                continue;
            };

            // Read the contents of the manifest file.
            let contents = provenance.read()?.map(Arc::<str>::from);

            // Cheap check to see if the manifest contains a pixi section.
            if let ManifestSource::PyProjectToml(source) = &contents {
                if !source.contains("[tool.pixi") {
                    continue;
                }
            }

            let path_diff = pathdiff::diff_paths(&provenance.path, self.current_path.as_path())
                .unwrap_or_else(|| manifest_dir_path.to_path_buf());
            let file_name = path_diff.to_string_lossy();
            let source = contents.into_named(file_name);

            // Parse the TOML from the manifest
            let mut toml = match toml_span::parse(source.inner()) {
                Ok(toml) => toml,
                Err(e) => {
                    return Err(WithSourceCode {
                        error: TomlError::from(e),
                        source,
                    }
                    .into())
                }
            };

            // Parse the workspace manifest.
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
                            return Err(WithSourceCode {
                                error: TomlError::from(err),
                                source,
                            }
                            .into())
                        }
                    };

                    if manifest.has_workspace() {
                        // Parse the manifest as a workspace manifest if it contains a workspace
                        manifest.into_workspace_manifest(ExternalWorkspaceProperties::default())
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
                            return Err(WithSourceCode {
                                error: TomlError::from(err),
                                source,
                            }
                            .into())
                        }
                    };

                    if manifest.has_pixi_workspace() {
                        // Parse the manifest as a workspace manifest if it
                        // contains a workspace
                        manifest.into_workspace_manifest()
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
                Err(error) => return Err(WithSourceCode { error, source }.into()),
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
                    let package_manifest = match package_manifest {
                        EitherManifest::Pixi(manifest) => manifest.into_package_manifest(
                            ExternalPackageProperties::default(),
                            &workspace_manifest,
                        ),
                        EitherManifest::Pyproject(manifest) => {
                            manifest.into_package_manifest(&workspace_manifest)
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
                            return Err(WithSourceCode { error, source }.into());
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
}

#[cfg(test)]
mod test {
    use std::{fmt::Write, path::Path};

    use rstest::*;

    use super::*;
    use crate::utils::test_utils::format_diagnostic;

    #[rstest]
    #[case::root("")]
    #[case::non_existing("non-existing")]
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
        let test_data_root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/workspace-discovery");

        let snapshot = match WorkspaceDiscoverer::new(test_data_root.join(subdir))
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
}
