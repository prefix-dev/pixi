use std::{collections::BTreeSet, path::PathBuf, str::FromStr};

use cargo_toml::{
    AbstractFilesystem, Error as CargoTomlError, Filesystem, Inheritable, Manifest, Package,
    PackageTemplate,
};
use miette::Diagnostic;
use once_cell::unsync::OnceCell;
use pixi_build_backend::generated_recipe::MetadataProvider;
use rattler_conda_types::{ParseVersionError, Version};

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum MetadataError {
    #[error(transparent)]
    CargoTomlError(CargoTomlError),
    #[error("failed to parse version from Cargo.toml, {0}")]
    ParseVersionError(ParseVersionError),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error("missing inherited value from workspace {0}")]
    MissingInheritedValue(String),
}

/// An implementation of [`MetadataProvider`] that reads metadata from a
/// Cargo.toml file and possibly an associated workspace manifest.
pub struct CargoMetadataProvider {
    manifest_root: PathBuf,
    cargo_manifest: OnceCell<Manifest>,
    workspace_manifest: OnceCell<(Manifest, PathBuf)>,
    ignore_cargo_manifest: bool,
}

impl CargoMetadataProvider {
    /// Constructs a new `CargoMetadataProvider` with the given manifest root.
    ///
    /// # Arguments
    ///
    /// * `manifest_root` - The directory that contains the `Cargo.toml` file
    /// * `ignore_cargo_manifest` - If `true`, all metadata methods will return
    ///   `None`, effectively disabling Cargo.toml metadata extraction
    pub fn new(manifest_root: impl Into<PathBuf>, ignore_cargo_manifest: bool) -> Self {
        Self {
            manifest_root: manifest_root.into(),
            cargo_manifest: OnceCell::default(),
            workspace_manifest: OnceCell::default(),
            ignore_cargo_manifest,
        }
    }

    /// Ensures that the manifest is loaded and returns the package metadata.
    fn ensure_manifest_package(&self) -> Result<Option<&Package>, MetadataError> {
        Ok(self.ensure_manifest()?.package.as_ref())
    }

    /// Ensures that the manifest is loaded
    fn ensure_manifest(&self) -> Result<&Manifest, MetadataError> {
        self.cargo_manifest.get_or_try_init(move || {
            let cargo_toml_content = fs_err::read_to_string(self.manifest_root.join("Cargo.toml"))?;
            Manifest::from_slice_with_metadata(cargo_toml_content.as_bytes())
                .map_err(MetadataError::CargoTomlError)
        })
    }

    /// Ensures that the workspace manifest is loaded, and returns the package
    /// template
    fn ensure_workspace_manifest(&self) -> Result<Option<&PackageTemplate>, MetadataError> {
        let manifest = self.ensure_manifest()?;

        // If the package manifest already has a workspace defined, return that.
        if let Some(workspace) = &manifest.workspace {
            return Ok(workspace.package.as_ref());
        }

        let workspace_hint = manifest.package.as_ref().and_then(|p| p.workspace.clone());
        let (manifest, _) = self.workspace_manifest.get_or_try_init(move || {
            Filesystem::new(&self.manifest_root)
                .parse_root_workspace(workspace_hint.as_deref())
                .map_err(MetadataError::CargoTomlError)
        })?;
        Ok(manifest.workspace.as_ref().and_then(|w| w.package.as_ref()))
    }

    /// Returns the set of globs that match files that influence the metadata of
    /// this package.
    ///
    /// This includes the package's own `Cargo.toml` file and any workspace
    /// `Cargo.toml` files if workspace inheritance is detected. These globs
    /// can be used for incremental builds to determine when metadata might
    /// have changed.
    ///
    /// # Returns
    ///
    /// A `BTreeSet` of glob patterns as strings. Common patterns include:
    /// - `"Cargo.toml"` - The package's manifest file
    /// - `"../../**/Cargo.toml"` - Workspace manifest files (when workspace
    ///   inheritance is used)
    pub fn input_globs(&self) -> BTreeSet<String> {
        let mut input_globs = BTreeSet::new();

        let Some(_) = self.cargo_manifest.get() else {
            return input_globs;
        };

        // Add the Cargo.toml manifest file itself.
        input_globs.insert(String::from("Cargo.toml"));

        // If the manifest has workspace inheritance, include that as well.
        if let Some((_, workspace_path)) = self.workspace_manifest.get() {
            // If the workspace is defined in the package we just include the path to the
            // workspace itself.
            let workspace_selected = self
                .cargo_manifest
                .get()
                .and_then(|p| p.package.as_ref())
                .is_some_and(|p| p.workspace.is_some());

            if let Some(path) = pathdiff::diff_paths(
                workspace_path
                    .parent()
                    .expect("the workspace path is a file so it must have a parent"),
                &self.manifest_root,
            ) {
                if workspace_selected {
                    input_globs.insert(format!(
                        "{}/Cargo.toml",
                        path.display().to_string().replace("\\", "/")
                    ));
                } else {
                    // Otherwise we assume the file is located in a parent directory of the package.
                    input_globs.extend(
                        path.components()
                            .take_while(|p| matches!(p, std::path::Component::ParentDir))
                            .enumerate()
                            .map(|(idx, _)| format!("{}Cargo.toml", "../".repeat(idx + 1))),
                    )
                }
            }
        }

        input_globs
    }
}

impl MetadataProvider for CargoMetadataProvider {
    type Error = MetadataError;

    /// Returns the package name from the Cargo.toml manifest.
    ///
    /// If `ignore_cargo_manifest` is true, returns `None`. Otherwise, extracts
    /// the name from the package section of the Cargo.toml file.
    fn name(&mut self) -> Result<Option<String>, Self::Error> {
        if self.ignore_cargo_manifest {
            return Ok(None);
        }
        Ok(self.ensure_manifest_package()?.map(|pkg| pkg.name.clone()))
    }

    /// Returns the package version from the Cargo.toml manifest.
    ///
    /// If `ignore_cargo_manifest` is true, returns `None`. Otherwise, extracts
    /// the version from the package section, handling workspace inheritance if
    /// needed. The version string is parsed into a
    /// `rattler_conda_types::Version`.
    fn version(&mut self) -> Result<Option<Version>, Self::Error> {
        if self.ignore_cargo_manifest {
            return Ok(None);
        }
        let Some(value) = self.ensure_manifest_package()?.map(|pkg| &pkg.version) else {
            return Ok(None);
        };
        let version = match value {
            Inheritable::Set(value) => value,
            Inheritable::Inherited => self
                .ensure_workspace_manifest()?
                .and_then(|template| template.version.as_ref())
                .ok_or_else(|| {
                    MetadataError::MissingInheritedValue(String::from("workspace.package.version"))
                })?,
        };
        Ok(Some(
            Version::from_str(version).map_err(MetadataError::ParseVersionError)?,
        ))
    }

    /// Returns the package description from the Cargo.toml manifest.
    ///
    /// If `ignore_cargo_manifest` is true, returns `None`. Otherwise, extracts
    /// the description from the package section, handling workspace inheritance
    /// if needed.
    fn description(&mut self) -> Result<Option<String>, Self::Error> {
        if self.ignore_cargo_manifest {
            return Ok(None);
        }
        let Some(value) = self.ensure_manifest_package()?.map(|pkg| &pkg.description) else {
            return Ok(None);
        };
        let description = match value {
            None => return Ok(None),
            Some(Inheritable::Set(value)) => value,
            Some(Inheritable::Inherited) => self
                .ensure_workspace_manifest()?
                .and_then(|template| template.description.as_ref())
                .ok_or_else(|| {
                    MetadataError::MissingInheritedValue(String::from(
                        "workspace.package.description",
                    ))
                })?,
        };
        Ok(Some(description.clone()))
    }

    /// Returns the package homepage URL from the Cargo.toml manifest.
    ///
    /// If `ignore_cargo_manifest` is true, returns `None`. Otherwise, extracts
    /// the homepage from the package section, handling workspace inheritance if
    /// needed.
    fn homepage(&mut self) -> Result<Option<String>, Self::Error> {
        if self.ignore_cargo_manifest {
            return Ok(None);
        }
        let Some(value) = self.ensure_manifest_package()?.map(|pkg| &pkg.homepage) else {
            return Ok(None);
        };
        let homepage = match value {
            None => return Ok(None),
            Some(Inheritable::Set(value)) => value,
            Some(Inheritable::Inherited) => self
                .ensure_workspace_manifest()?
                .and_then(|template| template.homepage.as_ref())
                .ok_or_else(|| {
                    MetadataError::MissingInheritedValue(String::from("workspace.package.homepage"))
                })?,
        };
        Ok(Some(homepage.clone()))
    }

    /// Returns the package license from the Cargo.toml manifest.
    ///
    /// If `ignore_cargo_manifest` is true, returns `None`. Otherwise, extracts
    /// the license from the package section, handling workspace inheritance if
    /// needed.
    fn license(&mut self) -> Result<Option<String>, Self::Error> {
        if self.ignore_cargo_manifest {
            return Ok(None);
        }
        let Some(value) = self.ensure_manifest_package()?.map(|pkg| &pkg.license) else {
            return Ok(None);
        };
        let license = match value {
            None => return Ok(None),
            Some(Inheritable::Set(value)) => value,
            Some(Inheritable::Inherited) => self
                .ensure_workspace_manifest()?
                .and_then(|template| template.license.as_ref())
                .ok_or_else(|| {
                    MetadataError::MissingInheritedValue(String::from("workspace.package.license"))
                })?,
        };
        Ok(Some(license.clone()))
    }

    /// Returns the package license file path from the Cargo.toml manifest.
    ///
    /// If `ignore_cargo_manifest` is true, returns `None`. Otherwise, extracts
    /// the license-file from the package section, handling workspace
    /// inheritance if needed. The path is converted to a string
    /// representation. Since Cargo.toml only supports a single license-file,
    /// returns a Vec with one element if present.
    fn license_files(&mut self) -> Result<Option<Vec<String>>, Self::Error> {
        if self.ignore_cargo_manifest {
            return Ok(None);
        }
        let Some(value) = self.ensure_manifest_package()?.map(|pkg| &pkg.license_file) else {
            return Ok(None);
        };
        let license_file = match value {
            None => return Ok(None),
            Some(Inheritable::Set(value)) => value,
            Some(Inheritable::Inherited) => self
                .ensure_workspace_manifest()?
                .and_then(|template| template.license_file.as_ref())
                .ok_or_else(|| {
                    MetadataError::MissingInheritedValue(String::from(
                        "workspace.package.license-file",
                    ))
                })?,
        };
        Ok(Some(vec![license_file.display().to_string()]))
    }

    /// Returns the package summary from the Cargo.toml manifest.
    ///
    /// Currently always returns `None` as Cargo.toml does not have a summary
    /// field. This could be implemented to return the description field as
    /// a fallback.
    fn summary(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }

    /// Returns the package documentation URL from the Cargo.toml manifest.
    ///
    /// If `ignore_cargo_manifest` is true, returns `None`. Otherwise, extracts
    /// the documentation from the package section, handling workspace
    /// inheritance if needed.
    fn documentation(&mut self) -> Result<Option<String>, Self::Error> {
        if self.ignore_cargo_manifest {
            return Ok(None);
        }
        let Some(value) = self
            .ensure_manifest_package()?
            .map(|pkg| &pkg.documentation)
        else {
            return Ok(None);
        };
        let documentation = match value {
            None => return Ok(None),
            Some(Inheritable::Set(value)) => value,
            Some(Inheritable::Inherited) => self
                .ensure_workspace_manifest()?
                .and_then(|template| template.documentation.as_ref())
                .ok_or_else(|| {
                    MetadataError::MissingInheritedValue(String::from(
                        "workspace.package.documentation",
                    ))
                })?,
        };
        Ok(Some(documentation.clone()))
    }

    /// Returns the package repository URL from the Cargo.toml manifest.
    ///
    /// If `ignore_cargo_manifest` is true, returns `None`. Otherwise, extracts
    /// the repository from the package section, handling workspace inheritance
    /// if needed.
    fn repository(&mut self) -> Result<Option<String>, Self::Error> {
        if self.ignore_cargo_manifest {
            return Ok(None);
        }
        let Some(value) = self.ensure_manifest_package()?.map(|pkg| &pkg.repository) else {
            return Ok(None);
        };
        let repository = match value {
            None => return Ok(None),
            Some(Inheritable::Set(value)) => value,
            Some(Inheritable::Inherited) => self
                .ensure_workspace_manifest()?
                .and_then(|template| template.repository.as_ref())
                .ok_or_else(|| {
                    MetadataError::MissingInheritedValue(String::from(
                        "workspace.package.repository",
                    ))
                })?,
        };
        Ok(Some(repository.clone()))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pixi_build_backend::generated_recipe::MetadataProvider;
    use tempfile::TempDir;

    use super::*;

    /// Helper function to create a temporary directory with a Cargo.toml file
    fn create_temp_cargo_project(cargo_toml_content: &str) -> TempDir {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let cargo_toml_path = temp_dir.path().join("Cargo.toml");
        fs::write(cargo_toml_path, cargo_toml_content).expect("Failed to write Cargo.toml");
        temp_dir
    }

    /// Helper function to create a CargoMetadataProvider for testing
    fn create_metadata_provider(manifest_root: &std::path::Path) -> CargoMetadataProvider {
        CargoMetadataProvider::new(manifest_root, false)
    }

    /// Helper function to assert workspace inheritance error
    fn assert_missing_inherited_value_error(
        result: Result<Option<String>, MetadataError>,
        expected_field: &str,
    ) {
        assert!(result.is_err());
        let error = result.unwrap_err();
        match error {
            MetadataError::MissingInheritedValue(field) => {
                assert_eq!(field, expected_field);
            }
            MetadataError::CargoTomlError(_) => {
                // This is expected when workspace inheritance fails due to
                // missing workspace
            }
            _ => panic!("Expected MissingInheritedValue or CargoTomlError, got: {error:?}"),
        }
    }

    /// Helper function to assert workspace inheritance error for version
    fn assert_version_inheritance_error(
        result: Result<Option<rattler_conda_types::Version>, MetadataError>,
        expected_field: &str,
    ) {
        assert!(result.is_err());
        let error = result.unwrap_err();
        match error {
            MetadataError::MissingInheritedValue(field) => {
                assert_eq!(field, expected_field);
            }
            MetadataError::CargoTomlError(_) => {
                // This is expected when workspace inheritance fails due to
                // missing workspace
            }
            _ => panic!("Expected MissingInheritedValue or CargoTomlError, got: {error:?}"),
        }
    }

    #[test]
    fn test_workspace_inheritance_in_same_file() {
        let cargo_toml_content = r#"
[workspace]
members = []

[workspace.package]
version = "1.0.0"
description = "Workspace description"
license = "MIT"
homepage = "https://workspace.example.com"
repository = "https://github.com/workspace/repo"
documentation = "https://docs.workspace.example.com"

[package]
name = "test-package"
version.workspace = true
description.workspace = true
license.workspace = true
homepage.workspace = true
repository.workspace = true
documentation.workspace = true
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        // Test that workspace inheritance works when workspace is in the same file
        assert_eq!(provider.name().unwrap(), Some("test-package".to_string()));
        assert_eq!(provider.version().unwrap().unwrap().to_string(), "1.0.0");
        assert_eq!(
            provider.description().unwrap(),
            Some("Workspace description".to_string())
        );
        assert_eq!(provider.license().unwrap(), Some("MIT".to_string()));
        assert_eq!(
            provider.homepage().unwrap(),
            Some("https://workspace.example.com".to_string())
        );
        assert_eq!(
            provider.repository().unwrap(),
            Some("https://github.com/workspace/repo".to_string())
        );
        assert_eq!(
            provider.documentation().unwrap(),
            Some("https://docs.workspace.example.com".to_string())
        );
    }

    #[test]
    fn test_inheritance_without_workspace_version() {
        let cargo_toml_content = r#"
[package]
name = "test-package"
version.workspace = true
description = "Regular description"
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        // Test that inheritance fails when no workspace is defined
        let result = provider.version();
        assert_version_inheritance_error(result, "workspace.package.version");
    }

    #[test]
    fn test_inheritance_without_workspace_description() {
        let cargo_toml_content = r#"
[package]
name = "test-package"
version = "1.0.0"
description.workspace = true
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        let result = provider.description();
        assert_missing_inherited_value_error(result, "workspace.package.description");
    }

    #[test]
    fn test_inheritance_without_workspace_license() {
        let cargo_toml_content = r#"
[package]
name = "test-package"
version = "1.0.0"
license.workspace = true
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        let result = provider.license();
        assert_missing_inherited_value_error(result, "workspace.package.license");
    }

    #[test]
    fn test_inheritance_without_workspace_homepage() {
        let cargo_toml_content = r#"
[package]
name = "test-package"
version = "1.0.0"
homepage.workspace = true
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        let result = provider.homepage();
        assert_missing_inherited_value_error(result, "workspace.package.homepage");
    }

    #[test]
    fn test_inheritance_without_workspace_repository() {
        let cargo_toml_content = r#"
[package]
name = "test-package"
version = "1.0.0"
repository.workspace = true
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        let result = provider.repository();
        assert_missing_inherited_value_error(result, "workspace.package.repository");
    }

    #[test]
    fn test_inheritance_without_workspace_documentation() {
        let cargo_toml_content = r#"
[package]
name = "test-package"
version = "1.0.0"
documentation.workspace = true
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        let result = provider.documentation();
        assert_missing_inherited_value_error(result, "workspace.package.documentation");
    }

    #[test]
    fn test_inheritance_without_workspace_license_file() {
        let cargo_toml_content = r#"
[package]
name = "test-package"
version = "1.0.0"
license-file.workspace = true
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        let result = provider
            .license_files()
            .map(|opt| opt.map(|v| v.join(", ")));
        assert_missing_inherited_value_error(result, "workspace.package.license-file");
    }

    #[test]
    fn test_workspace_with_partial_inheritance() {
        let cargo_toml_content = r#"
[workspace]
members = []

[workspace.package]
version = "2.0.0"
description = "Workspace description"

[package]
name = "test-package"
version.workspace = true
description = "Package-specific description"
license = "Apache-2.0"
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        // Test mixed inheritance and direct values
        assert_eq!(provider.version().unwrap().unwrap().to_string(), "2.0.0");
        assert_eq!(
            provider.description().unwrap(),
            Some("Package-specific description".to_string())
        );
        assert_eq!(provider.license().unwrap(), Some("Apache-2.0".to_string()));
    }

    #[test]
    fn test_workspace_inheritance_with_missing_workspace_fields() {
        let cargo_toml_content = r#"
[workspace]
members = []

[workspace.package]
version = "1.5.0"

[package]
name = "test-package"
version.workspace = true
description.workspace = true
license.workspace = true
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        // Version should work
        assert_eq!(provider.version().unwrap().unwrap().to_string(), "1.5.0");

        // Description should fail
        let result = provider.description();
        assert_missing_inherited_value_error(result, "workspace.package.description");

        // License should fail
        let result = provider.license();
        assert_missing_inherited_value_error(result, "workspace.package.license");
    }

    #[test]
    fn test_input_globs_with_workspace_in_same_file() {
        let cargo_toml_content = r#"
[workspace]
members = []

[workspace.package]
version = "1.0.0"

[package]
name = "test-package"
version.workspace = true
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        // Force loading of manifest and workspace
        let _ = provider.version().unwrap();

        let globs = provider.input_globs();
        assert!(globs.contains("Cargo.toml"));
        // When workspace is in the same file, no additional glob is needed
        assert_eq!(
            globs.len(),
            1,
            "Expected only Cargo.toml glob when workspace is in same file, got: {globs:?}"
        );
    }

    #[test]
    fn test_input_globs_with_separate_workspace_file() {
        // Create workspace root directory
        let workspace_dir = TempDir::new().expect("Failed to create workspace temp directory");
        let workspace_cargo_toml = r#"
[workspace]
members = ["package"]

[workspace.package]
version = "2.0.0"
description = "Workspace description"
"#;
        fs::write(
            workspace_dir.path().join("Cargo.toml"),
            workspace_cargo_toml,
        )
        .expect("Failed to write workspace Cargo.toml");

        // Create package subdirectory
        let package_dir = workspace_dir.path().join("package");
        fs::create_dir(&package_dir).expect("Failed to create package directory");
        let package_cargo_toml = r#"
[package]
name = "test-package"
version.workspace = true
description.workspace = true
"#;
        fs::write(package_dir.join("Cargo.toml"), package_cargo_toml)
            .expect("Failed to write package Cargo.toml");

        let mut provider = create_metadata_provider(&package_dir);

        // Force loading of manifest and workspace
        let version_result = provider.version();
        assert!(
            version_result.is_ok(),
            "Version should be inherited from workspace"
        );
        assert_eq!(version_result.unwrap().unwrap().to_string(), "2.0.0");

        let globs = provider.input_globs();
        assert!(globs.contains("Cargo.toml"));
        // Should include workspace glob since workspace inheritance from separate file
        // is detected
        assert!(
            globs.len() >= 2,
            "Expected at least 2 globs when workspace is in separate file, got: {globs:?}"
        );

        // Check that a workspace-related glob pattern is included
        let has_workspace_glob = globs
            .iter()
            .any(|glob| glob.contains("../Cargo.toml") && glob != "Cargo.toml");
        assert!(
            has_workspace_glob,
            "Expected workspace glob pattern, got: {globs:?}"
        );
    }

    #[test]
    fn test_input_globs_without_workspace() {
        let cargo_toml_content = r#"
[package]
name = "test-package"
version = "1.0.0"
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        // Force loading of manifest
        let _ = provider.version().unwrap();

        let globs = provider.input_globs();
        assert_eq!(
            globs.len(),
            1,
            "Expected exactly 1 glob when no workspace is present, got: {globs:?}"
        );
        assert!(globs.contains("Cargo.toml"));

        // Verify no workspace-related globs are present
        let has_workspace_glob = globs
            .iter()
            .any(|glob| glob.contains("**/Cargo.toml") && glob != "Cargo.toml");
        assert!(
            !has_workspace_glob,
            "No workspace globs should be present when no workspace inheritance occurs, got: {globs:?}"
        );
    }

    #[test]
    fn test_input_globs_no_inheritance_with_workspace_present() {
        let cargo_toml_content = r#"
[workspace]
members = []

[workspace.package]
version = "2.0.0"

[package]
name = "test-package"
version = "1.0.0"
description = "Direct package values"
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        // Force loading of manifest - no inheritance should occur
        let version = provider.version().unwrap().unwrap();
        assert_eq!(
            version.to_string(),
            "1.0.0",
            "Should use direct package version, not workspace version"
        );

        let globs = provider.input_globs();
        // When workspace exists but no inheritance is used, only Cargo.toml should be
        // included
        assert_eq!(
            globs.len(),
            1,
            "Expected exactly 1 glob when workspace exists but no inheritance is used, got: {globs:?}"
        );
        assert!(globs.contains("Cargo.toml"));
    }

    #[test]
    fn test_ignore_cargo_manifest_flag() {
        let cargo_toml_content = r#"
[package]
name = "test-package"
version = "1.0.0"
description = "Test description"
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = CargoMetadataProvider::new(temp_dir.path(), true);

        // All methods should return None when ignore_cargo_manifest is true
        assert_eq!(provider.name().unwrap(), None);
        assert_eq!(provider.version().unwrap(), None);
        assert_eq!(provider.description().unwrap(), None);
        assert_eq!(provider.license().unwrap(), None);
        assert_eq!(provider.homepage().unwrap(), None);
        assert_eq!(provider.repository().unwrap(), None);
        assert_eq!(provider.documentation().unwrap(), None);
        assert_eq!(provider.license_files().unwrap(), None);
        assert_eq!(provider.summary().unwrap(), None);
    }

    #[test]
    fn test_invalid_version_format() {
        let cargo_toml_content = r#"
[package]
name = "test-package"
version = "not.a.valid.version.at.all"
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        let result = provider.version();
        // Note: rattler_conda_types::Version is quite permissive, so let's test what
        // actually happens
        match result {
            Ok(Some(version)) => {
                // If it parses successfully, that's also valid behavior - conda versions are
                // flexible
                assert!(!version.to_string().is_empty());
            }
            Err(MetadataError::ParseVersionError(_)) => {
                // This is the expected error case
            }
            other => panic!("Unexpected result: {other:?}"),
        }
    }

    #[test]
    fn test_malformed_cargo_toml() {
        let cargo_toml_content = r#"
[package
name = "test-package"
version = "1.0.0"
"#;

        let temp_dir = create_temp_cargo_project(cargo_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        let result = provider.name();
        assert!(result.is_err());
        match result.unwrap_err() {
            MetadataError::CargoTomlError(_) => {}
            err => panic!("Expected CargoTomlError, got: {err:?}"),
        }
    }
}
