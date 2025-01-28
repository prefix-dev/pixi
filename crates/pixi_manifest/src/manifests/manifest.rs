use std::path::Path;

use itertools::Itertools;
use miette::{IntoDiagnostic, NamedSource, Report};
use toml_edit::DocumentMut;

use crate::{
    error::TomlError,
    manifests::{provenance::ManifestKind, PackageManifest},
    pyproject::{PyProjectManifest},
    toml::{ExternalWorkspaceProperties, FromTomlStr, TomlManifest},
    PackageBuild, WorkspaceManifest,
};

/// Contains information about a manifest file.
///
/// This struct is responsible for reading, parsing, editing, and saving the
/// manifest. It encapsulates all logic related to the manifest's TOML format
/// and structure. The manifest data is represented as a [`WorkspaceManifest`]
/// struct for easy manipulation.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The parsed workspace manifest
    pub workspace: WorkspaceManifest,

    /// Optionally a package manifest
    pub package: Option<PackageManifest>,
}

impl Manifest {
    /// Create a new manifest from a path
    pub fn from_path(path: impl AsRef<Path>) -> miette::Result<Self> {
        let manifest_path = dunce::canonicalize(path.as_ref()).into_diagnostic()?;
        let contents = fs_err::read_to_string(path.as_ref()).into_diagnostic()?;
        Self::from_str(manifest_path.as_ref(), contents)
    }

    /// Create a new manifest from a string
    pub fn from_str(manifest_path: &Path, contents: impl Into<String>) -> miette::Result<Self> {
        let manifest_kind = ManifestKind::try_from_path(manifest_path).ok_or_else(|| {
            miette::miette!("unrecognized manifest file: {}", manifest_path.display())
        })?;
        let root = manifest_path
            .parent()
            .expect("manifest_path should always have a parent");

        let contents = contents.into();
        let (parsed, file_name) = match manifest_kind {
            ManifestKind::Pixi => (
                TomlManifest::from_toml_str(&contents).and_then(|manifest| {
                    manifest.into_workspace_manifest(ExternalWorkspaceProperties::default())
                }),
                "pixi.toml",
            ),
            ManifestKind::Pyproject => {
                let manifest = match PyProjectManifest::from_toml_str(&contents)
                    .and_then(|m| m.ensure_pixi())
                {
                    Ok(manifest) => match manifest.into_manifests() {
                        Ok(manifests) => Ok(manifests),
                        Err(e) => return Err(Report::from(e)),
                    },
                    Err(e) => Err(e),
                };
                (manifest, "pyproject.toml")
            }
        };

        let ((workspace_manifest, package_manifest, warnings), _) =
            match parsed.and_then(|manifest| {
                contents
                    .parse::<DocumentMut>()
                    .map(|doc| (manifest, doc))
                    .map_err(TomlError::from)
            }) {
                Ok(result) => result,
                Err(e) => {
                    return Err(Report::from(e)
                        .with_source_code(NamedSource::new(file_name, contents.clone())));
                }
            };

        // Validate the contents of the manifest
        workspace_manifest.validate(NamedSource::new(file_name, contents.to_owned()), root)?;

        // Emit warnings
        if !warnings.is_empty() {
            tracing::warn!(
                "Encountered {} warning{} while parsing the manifest:\n{}",
                warnings.len(),
                if warnings.len() == 1 { "" } else { "s" },
                warnings
                    .into_iter()
                    .map(|warning| Report::from(warning)
                        .with_source_code(NamedSource::new(file_name, contents.clone())))
                    .format_with("\n", |w, f| f(&format_args!("{:?}", w)))
            );
        }

        Ok(Self {
            workspace: workspace_manifest,
            package: package_manifest,
        })
    }

    /// Return the build section from the parsed manifest
    pub fn build_section(&self) -> Option<&PackageBuild> {
        self.package.as_ref().map(|package| &package.build)
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use rattler_conda_types::Version;
    use rstest::*;
    use tempfile::tempdir;

    use super::*;

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = ["linux-64", "win-64", "osx-64"]
        "#;

    #[test]
    fn test_from_path() {
        // Test the toml from a path
        let dir = tempdir().unwrap();
        let path = dir.path().join("pixi.toml");
        fs_err::write(&path, PROJECT_BOILERPLATE).unwrap();
        // From &PathBuf
        let _manifest = Manifest::from_path(&path).unwrap();
        // From &Path
        let _manifest = Manifest::from_path(path.as_path()).unwrap();
        // From PathBuf
        let manifest = Manifest::from_path(path).unwrap();

        assert_eq!(manifest.workspace.workspace.name, "foo");
        assert_eq!(
            manifest.workspace.workspace.version,
            Some(Version::from_str("0.1.0").unwrap())
        );
    }

    #[rstest]
    fn test_docs_pixi_manifests(
        #[files("../../docs/source_files/pixi_tomls/*.toml")] manifest_path: PathBuf,
    ) {
        let contents = fs_err::read_to_string(manifest_path).unwrap();
        let _manifest = Manifest::from_str(Path::new("pixi.toml"), contents).unwrap();
    }
}
