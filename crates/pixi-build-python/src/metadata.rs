use std::{cell::RefCell, collections::BTreeSet, path::PathBuf, str::FromStr};

use miette::Diagnostic;
use once_cell::unsync::OnceCell;
use pixi_build_backend::generated_recipe::MetadataProvider;
use pyproject_toml::PyProjectToml;
use rattler_conda_types::{ParseVersionError, Version};

/// Controls how the `PyprojectMetadataProvider` handles the pyproject.toml manifest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PyprojectManifestMode {
    /// Read metadata from the pyproject.toml file.
    #[default]
    Read,
    /// Ignore the pyproject.toml file; all metadata methods will return `None`.
    Ignore,
}

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum MetadataError {
    #[error("failed to parse pyproject.toml, {0}")]
    PyProjectToml(#[from] toml::de::Error),
    #[error("failed to parse version from pyproject.toml, {0}")]
    ParseVersion(ParseVersionError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// An implementation of [`MetadataProvider`] that reads metadata from a
/// pyproject.toml file.
pub struct PyprojectMetadataProvider {
    manifest_root: PathBuf,
    pyproject_manifest: OnceCell<PyProjectToml>,
    mode: PyprojectManifestMode,
    warnings: RefCell<Vec<String>>,
}

impl PyprojectMetadataProvider {
    /// Constructs a new `PyprojectMetadataProvider` with the given manifest root.
    ///
    /// # Arguments
    ///
    /// * `manifest_root` - The directory that contains the `pyproject.toml` file
    /// * `mode` - Controls whether to read metadata from or ignore the pyproject.toml
    pub fn new(manifest_root: impl Into<PathBuf>, mode: PyprojectManifestMode) -> Self {
        Self {
            manifest_root: manifest_root.into(),
            pyproject_manifest: OnceCell::default(),
            mode,
            warnings: RefCell::new(Vec::new()),
        }
    }

    /// Returns all warnings collected during metadata extraction.
    ///
    /// This includes warnings about invalid SPDX license expressions and other
    /// metadata parsing issues that don't cause errors but may indicate problems.
    pub fn warnings(&self) -> Vec<String> {
        self.warnings.borrow().clone()
    }

    /// Adds a warning message to the warning collection.
    fn add_warning(&self, warning: impl Into<String>) {
        self.warnings.borrow_mut().push(warning.into());
    }

    /// Ensures that the manifest is loaded and returns the project metadata.
    fn ensure_manifest_project(&self) -> Result<Option<&pyproject_toml::Project>, MetadataError> {
        Ok(self.ensure_manifest()?.project.as_ref())
    }

    /// Ensures that the manifest is loaded
    fn ensure_manifest(&self) -> Result<&PyProjectToml, MetadataError> {
        self.pyproject_manifest.get_or_try_init(move || {
            let pyproject_toml_content =
                fs_err::read_to_string(self.manifest_root.join("pyproject.toml"))?;
            toml::from_str(&pyproject_toml_content).map_err(MetadataError::PyProjectToml)
        })
    }

    /// Returns the set of globs that match files that influence the metadata of
    /// this package.
    ///
    /// This includes the package's own `pyproject.toml` file. These globs
    /// can be used for incremental builds to determine when metadata might
    /// have changed.
    ///
    /// # Returns
    ///
    /// A `BTreeSet` of glob patterns as strings. Common patterns include:
    /// - `"pyproject.toml"` - The package's manifest file
    pub fn input_globs(&self) -> BTreeSet<String> {
        let mut input_globs = BTreeSet::new();

        let Some(_) = self.pyproject_manifest.get() else {
            return input_globs;
        };

        // Add the pyproject.toml manifest file itself.
        input_globs.insert(String::from("pyproject.toml"));

        input_globs
    }
}

impl MetadataProvider for PyprojectMetadataProvider {
    type Error = MetadataError;

    /// Returns the package name from the pyproject.toml manifest.
    ///
    /// If `ignore_pyproject_manifest` is true, returns `None`. Otherwise, extracts
    /// the name from the project section of the pyproject.toml file.
    fn name(&mut self) -> Result<Option<String>, Self::Error> {
        if self.mode == PyprojectManifestMode::Ignore {
            return Ok(None);
        }
        Ok(self
            .ensure_manifest_project()?
            .map(|proj| proj.name.clone()))
    }

    /// Returns the package version from the pyproject.toml manifest.
    ///
    /// If `ignore_pyproject_manifest` is true, returns `None`. Otherwise, extracts
    /// the version from the project section. The version string is parsed into a
    /// `rattler_conda_types::Version`.
    fn version(&mut self) -> Result<Option<Version>, Self::Error> {
        if self.mode == PyprojectManifestMode::Ignore {
            return Ok(None);
        }
        let Some(project) = self.ensure_manifest_project()? else {
            return Ok(None);
        };
        let Some(version) = &project.version else {
            return Ok(None);
        };
        Ok(Some(
            Version::from_str(&version.to_string()).map_err(MetadataError::ParseVersion)?,
        ))
    }

    /// Returns the package description from the pyproject.toml manifest.
    ///
    /// If `ignore_pyproject_manifest` is true, returns `None`. Otherwise, extracts
    /// the description from the project section.
    fn description(&mut self) -> Result<Option<String>, Self::Error> {
        if self.mode == PyprojectManifestMode::Ignore {
            return Ok(None);
        }
        Ok(self
            .ensure_manifest_project()?
            .and_then(|proj| proj.description.clone()))
    }

    /// Returns the package homepage URL from the pyproject.toml manifest.
    ///
    /// If `ignore_pyproject_manifest` is true, returns `None`. Otherwise, extracts
    /// the homepage from the project.urls section.
    fn homepage(&mut self) -> Result<Option<String>, Self::Error> {
        if self.mode == PyprojectManifestMode::Ignore {
            return Ok(None);
        }
        Ok(self
            .ensure_manifest_project()?
            .and_then(|proj| proj.urls.as_ref())
            .and_then(|urls| urls.get("Homepage").cloned()))
    }

    /// Returns the package license from the pyproject.toml manifest.
    ///
    /// If `ignore_pyproject_manifest` is true, returns `None`. Otherwise, extracts
    /// the license from the project section. If the license text is not a valid
    /// SPDX expression, a warning is added and `None` is returned.
    fn license(&mut self) -> Result<Option<String>, Self::Error> {
        if self.mode == PyprojectManifestMode::Ignore {
            return Ok(None);
        }
        Ok(self
            .ensure_manifest_project()?
            .and_then(|proj| match proj.license.as_ref() {
                Some(pyproject_toml::License::Spdx(spdx)) => {
                    match spdx.parse::<spdx::Expression>() {
                        Ok(expr) => Some(expr.to_string()),
                        Err(err) => {
                            self.add_warning(format!(
                                "License '{}' is not a valid SPDX expression: {}. \
                                 Consider using a valid SPDX identifier (e.g., 'MIT', 'Apache-2.0'). \
                                 See <https://spdx.org/licenses> for the list of valid licenses.",
                                spdx, err
                            ));
                            None
                        }
                    }
                }
                Some(pyproject_toml::License::Text { text }) => {
                    match text.parse::<spdx::Expression>() {
                        Ok(expr) => Some(expr.to_string()),
                        Err(err) => {
                            self.add_warning(format!(
                                "License text '{}' is not a valid SPDX expression: {}. \
                                 Consider using a valid SPDX identifier (e.g., 'MIT', 'Apache-2.0'). \
                                 See <https://spdx.org/licenses> for the list of valid licenses.",
                                text, err
                            ));
                            None
                        }
                    }
                }
                _ => None,
            }))
    }

    /// Returns the package license file path(s) from the pyproject.toml manifest.
    ///
    /// If `ignore_pyproject_manifest` is true, returns `None`. Otherwise, extracts
    /// the license file path from the project section. This method checks:
    /// 1. `license.file` - if specified as a file reference
    /// 2. `license-files` - if specified as a list of file paths
    ///
    /// If both are present, they are combined with commas. If multiple files are
    /// present in `license-files`, they are joined with commas.
    fn license_files(&mut self) -> Result<Option<Vec<String>>, Self::Error> {
        if self.mode == PyprojectManifestMode::Ignore {
            return Ok(None);
        }

        let project = match self.ensure_manifest_project()? {
            Some(proj) => proj,
            None => return Ok(None),
        };

        let mut license_files = Vec::new();

        // Check for license.file
        if let Some(pyproject_toml::License::File { file }) = project.license.as_ref() {
            license_files.push(file.to_string_lossy().to_string());
        }

        // Check for license-files
        if let Some(files) = project.license_files.as_ref() {
            license_files.extend(files.iter().cloned());
        }

        if license_files.is_empty() {
            Ok(None)
        } else {
            Ok(Some(license_files))
        }
    }

    /// Returns the package summary from the pyproject.toml manifest.
    ///
    /// This returns the same as description since pyproject.toml doesn't have
    /// a separate summary field.
    fn summary(&mut self) -> Result<Option<String>, Self::Error> {
        self.description()
    }

    /// Returns the package documentation URL from the pyproject.toml manifest.
    ///
    /// If `ignore_pyproject_manifest` is true, returns `None`. Otherwise, extracts
    /// the documentation URL from the project.urls section.
    fn documentation(&mut self) -> Result<Option<String>, Self::Error> {
        if self.mode == PyprojectManifestMode::Ignore {
            return Ok(None);
        }
        Ok(self
            .ensure_manifest_project()?
            .and_then(|proj| proj.urls.as_ref())
            .and_then(|urls| {
                urls.get("Documentation")
                    .or_else(|| urls.get("Docs"))
                    .cloned()
            }))
    }

    /// Returns the package repository URL from the pyproject.toml manifest.
    ///
    /// If `ignore_pyproject_manifest` is true, returns `None`. Otherwise, extracts
    /// the repository URL from the project.urls section.
    fn repository(&mut self) -> Result<Option<String>, Self::Error> {
        if self.mode == PyprojectManifestMode::Ignore {
            return Ok(None);
        }
        Ok(self
            .ensure_manifest_project()?
            .and_then(|proj| proj.urls.as_ref())
            .and_then(|urls| {
                urls.get("Repository")
                    .or_else(|| urls.get("Source"))
                    .or_else(|| urls.get("Source Code"))
                    .cloned()
            }))
    }
}

impl PyprojectMetadataProvider {
    /// Returns the required Python version from the pyproject.toml manifest.
    ///
    /// If `ignore_pyproject_manifest` is true, returns `None`. Otherwise, extracts
    /// the requires-python from the project section.
    pub fn requires_python(&self) -> Result<Option<String>, MetadataError> {
        if self.mode == PyprojectManifestMode::Ignore {
            return Ok(None);
        }
        Ok(self
            .ensure_manifest_project()?
            .and_then(|proj| proj.requires_python.as_ref())
            .map(|req_py| req_py.to_string()))
    }

    /// Returns the project dependencies from the pyproject.toml manifest.
    ///
    /// If `ignore_pyproject_manifest` is true, returns `None`. Otherwise, extracts
    /// the dependencies from the `[project.dependencies]` section.
    pub fn project_dependencies(
        &self,
    ) -> Result<Option<&Vec<pep508_rs::Requirement<pep508_rs::VerbatimUrl>>>, MetadataError> {
        if self.mode == PyprojectManifestMode::Ignore {
            return Ok(None);
        }
        Ok(self
            .ensure_manifest_project()?
            .and_then(|proj| proj.dependencies.as_ref()))
    }

    /// Returns the build system requirements from the pyproject.toml manifest.
    ///
    /// If `ignore_pyproject_manifest` is true, returns `None`. Otherwise, extracts
    /// the requirements from the `[build-system].requires` section.
    pub fn build_system_requires(
        &self,
    ) -> Result<Option<&Vec<pep508_rs::Requirement<pep508_rs::VerbatimUrl>>>, MetadataError> {
        if self.mode == PyprojectManifestMode::Ignore {
            return Ok(None);
        }
        Ok(self
            .ensure_manifest()?
            .build_system
            .as_ref()
            .map(|bs| &bs.requires))
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, fs};

    use pixi_build_backend::generated_recipe::{GenerateRecipe, MetadataProvider};
    use rattler_conda_types::Platform;
    use tempfile::TempDir;

    use crate::{PythonGenerator, config::PythonBackendConfig, project_fixture};

    use super::*;

    /// Helper function to create a temporary directory with a pyproject.toml file
    fn create_temp_pyproject_project(pyproject_toml_content: &str) -> TempDir {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let pyproject_toml_path = temp_dir.path().join("pyproject.toml");
        fs::write(pyproject_toml_path, pyproject_toml_content)
            .expect("Failed to write pyproject.toml");
        temp_dir
    }

    /// Helper function to create a PyprojectMetadataProvider for testing
    fn create_metadata_provider(manifest_root: &std::path::Path) -> PyprojectMetadataProvider {
        PyprojectMetadataProvider::new(manifest_root, PyprojectManifestMode::Read)
    }

    #[test]
    fn test_basic_metadata_extraction() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
description = "A test package"
license = "MIT"

[project.urls]
Homepage = "https://example.com"
Repository = "https://github.com/example/test-package"
Documentation = "https://docs.example.com"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        assert_eq!(provider.name().unwrap(), Some("test-package".to_string()));
        assert_eq!(provider.version().unwrap().unwrap().to_string(), "1.0.0");
        assert_eq!(
            provider.description().unwrap(),
            Some("A test package".to_string())
        );
        assert_eq!(provider.license().unwrap(), Some("MIT".to_string()));
        assert_eq!(
            provider.homepage().unwrap(),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            provider.repository().unwrap(),
            Some("https://github.com/example/test-package".to_string())
        );
        assert_eq!(
            provider.documentation().unwrap(),
            Some("https://docs.example.com".to_string())
        );
    }

    #[test]
    fn test_license_from_file() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
license = {file = "LICENSE.txt"}
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        assert_eq!(provider.license().unwrap(), None);
        assert_eq!(
            provider.license_files().unwrap(),
            Some(vec!["LICENSE.txt".to_string()])
        );
    }

    #[test]
    fn test_license_files_field() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
license-files = ["LICENSE.txt", "COPYING.txt"]
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        assert_eq!(provider.license().unwrap(), None);
        assert_eq!(
            provider.license_files().unwrap(),
            Some(vec!["LICENSE.txt".to_string(), "COPYING.txt".to_string()])
        );
    }

    #[test]
    fn test_license_file_and_license_files_combined() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
license = {file = "LICENSE"}
license-files = ["NOTICE.txt", "AUTHORS.txt"]
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        assert_eq!(provider.license().unwrap(), None);
        assert_eq!(
            provider.license_files().unwrap(),
            Some(vec![
                "LICENSE".to_string(),
                "NOTICE.txt".to_string(),
                "AUTHORS.txt".to_string()
            ])
        );
    }

    #[test]
    fn test_single_license_file_in_license_files() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
license-files = ["LICENSE"]
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        assert_eq!(provider.license().unwrap(), None);
        assert_eq!(
            provider.license_files().unwrap(),
            Some(vec!["LICENSE".to_string()])
        );
    }

    #[test]
    fn test_license_from_text() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
license = {text = "MIT"}
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        assert_eq!(provider.license().unwrap(), Some("MIT".to_string()));
        assert_eq!(provider.license_files().unwrap(), None);

        // Verify that no warnings were generated for valid SPDX
        let warnings = provider.warnings();
        assert_eq!(warnings.len(), 0);
    }

    #[test]
    fn test_license_from_non_spdx_text() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
license = {text = "BLABLA"}
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        assert_eq!(provider.license().unwrap(), None);
        assert_eq!(provider.license_files().unwrap(), None);

        // Verify that a warning was generated
        let warnings = provider.warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("BLABLA"));
        assert!(warnings[0].contains("not a valid SPDX expression"));
    }

    #[test]
    fn test_missing_project_section() {
        let pyproject_toml_content = r#"
[build-system]
requires = ["setuptools", "wheel"]
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        assert_eq!(provider.name().unwrap(), None);
        assert_eq!(provider.version().unwrap(), None);
        assert_eq!(provider.description().unwrap(), None);
    }

    #[test]
    fn test_input_globs() {
        let pyproject_toml_content = r#"
    [project]
    name = "test-package"
    version = "1.0.0"
    "#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        // Force loading of manifest
        let _ = provider.name().unwrap();

        let globs = provider.input_globs();
        assert_eq!(globs.len(), 1);
        assert!(globs.contains("pyproject.toml"));
    }

    #[test]
    fn test_ignore_pyproject_manifest_flag() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
description = "Test description"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider =
            PyprojectMetadataProvider::new(temp_dir.path(), PyprojectManifestMode::Ignore);

        // All methods should return None when mode is Ignore
        assert_eq!(provider.name().unwrap(), None);
        assert_eq!(provider.version().unwrap(), None);
        assert_eq!(provider.description().unwrap(), None);
        assert_eq!(provider.license().unwrap(), None);
        assert_eq!(provider.homepage().unwrap(), None);
        assert_eq!(provider.repository().unwrap(), None);
        assert_eq!(provider.documentation().unwrap(), None);
        assert_eq!(provider.license_files().unwrap(), None);
        assert_eq!(provider.summary().unwrap(), None);
        assert_eq!(provider.requires_python().unwrap(), None);
    }

    #[test]
    fn test_alternative_url_keys() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"

[project.urls]
"Source Code" = "https://github.com/example/test-package"
Docs = "https://docs.example.com"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        assert_eq!(
            provider.repository().unwrap(),
            Some("https://github.com/example/test-package".to_string())
        );
        assert_eq!(
            provider.documentation().unwrap(),
            Some("https://docs.example.com".to_string())
        );
    }

    #[test]
    fn test_invalid_version_format() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0a1"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        // This should parse successfully since it's a valid PEP440 version
        let result = provider.version();
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn test_pyproject_toml_parse_error() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "not.a.valid.version.at.all"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        let result = provider.version();
        // The pyproject-toml parser should fail to parse this
        match result {
            Err(MetadataError::PyProjectToml(_)) => {
                // This is expected - invalid version in pyproject.toml
            }
            other => panic!("Expected PyProjectTomlError for invalid version, got: {other:?}"),
        }
    }

    #[test]
    fn test_malformed_pyproject_toml() {
        let pyproject_toml_content = r#"
[project
name = "test-package"
version = "1.0.0"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        let result = provider.name();
        assert!(result.is_err());
        match result.unwrap_err() {
            MetadataError::PyProjectToml(_) => {}
            err => panic!("Expected PyProjectToml, got: {err:?}"),
        }
    }

    #[test]
    fn test_summary_equals_description() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
description = "Test description"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let mut provider = create_metadata_provider(temp_dir.path());

        let description = provider.description().unwrap();
        let summary = provider.summary().unwrap();

        assert_eq!(description, summary);
        assert_eq!(summary, Some("Test description".to_string()));
    }

    #[test]
    fn test_requires_python_extraction() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
requires-python = ">=3.13"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let provider = create_metadata_provider(temp_dir.path());

        assert_eq!(
            provider.requires_python().unwrap(),
            Some(">=3.13".to_string())
        );
    }

    #[test]
    fn test_requires_python_with_ignore_flag() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
requires-python = ">=3.13"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let provider =
            PyprojectMetadataProvider::new(temp_dir.path(), PyprojectManifestMode::Ignore);

        assert_eq!(provider.requires_python().unwrap(), None);
    }

    #[test]
    fn test_build_system_requires_extraction() {
        let pyproject_toml_content = r#"
[build-system]
requires = ["flit_core<4", "setuptools>=42"]

[project]
name = "test-package"
version = "1.0.0"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let provider = create_metadata_provider(temp_dir.path());

        let requires = provider
            .build_system_requires()
            .expect("Should parse build-system.requires");
        assert!(requires.is_some(), "build-system.requires should exist");
        let requires = requires.unwrap();
        assert_eq!(requires.len(), 2);
        assert_eq!(requires[0].name.as_ref(), "flit-core");
        assert_eq!(
            requires[0].version_or_url.as_ref().unwrap().to_string(),
            "<4"
        );
        assert_eq!(requires[1].name.as_ref(), "setuptools");
        assert_eq!(
            requires[1].version_or_url.as_ref().unwrap().to_string(),
            ">=42"
        );
    }

    #[test]
    fn test_build_system_requires_with_ignore_flag() {
        let pyproject_toml_content = r#"
[build-system]
requires = ["flit_core<4"]

[project]
name = "test-package"
version = "1.0.0"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let provider =
            PyprojectMetadataProvider::new(temp_dir.path(), PyprojectManifestMode::Ignore);

        assert_eq!(provider.build_system_requires().unwrap(), None);
    }

    #[test]
    fn test_build_system_requires_missing() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        let provider = create_metadata_provider(temp_dir.path());

        let requires = provider
            .build_system_requires()
            .expect("Should not error when build-system is missing");
        assert!(
            requires.is_none(),
            "build-system.requires should be None when section is missing"
        );
    }

    #[tokio::test]
    async fn test_generated_recipe_contains_pyproject_values() {
        let pyproject_toml_content = r#"
[project]
name = "Test-package"
version = "99.0.0"
description = "A test package"
license = {text = "MIT"}

[project.urls]
Homepage = "https://example.com"
Repository = "https://github.com/example/test-package"
Documentation = "https://docs.example.com"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);
        // let mut provider = create_metadata_provider(temp_dir.path());

        // Now create project model and generate a recipe from it
        let project_model = project_fixture!({
            "targets": {
                "defaultTarget": {
                    "runDependencies": {
                        "boltons": {
                            "binary": {
                                "version": "*"
                            }
                        }
                    }
                },
            }
        });

        let generated_recipe = PythonGenerator::default()
            .generate_recipe(
                &project_model,
                // when using the default here we should read values from the pyproject.toml
                &PythonBackendConfig::default(),
                temp_dir.path().to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        insta::assert_yaml_snapshot!(generated_recipe.recipe, {
        ".source[0].path" => "[ ... path ... ]",
        ".build.script" => "[ ... script ... ]",
        });
    }

    #[tokio::test]
    async fn test_generated_recipe_respects_requires_python() {
        let pyproject_toml_content = r#"
[project]
name = "test-package"
version = "1.0.0"
requires-python = ">=3.13"
"#;

        let temp_dir = create_temp_pyproject_project(pyproject_toml_content);

        // Now create project model and generate a recipe from it
        let project_model = project_fixture!({
            "name": "foobar",
            "targets": {
                "defaultTarget": {
                    "runDependencies": {
                        "boltons": {
                            "binary": {
                                "version": "*"
                            }
                        }
                    }
                },
            }
        });

        let generated_recipe = PythonGenerator::default()
            .generate_recipe(
                &project_model,
                // when using the default here we should read values from the pyproject.toml
                &PythonBackendConfig::default(),
                temp_dir.path().to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // Check that Python requirements include the version constraint
        let host_requirements = &generated_recipe.recipe.requirements.host;
        let run_requirements = &generated_recipe.recipe.requirements.run;

        let has_python_constraint_host = host_requirements
            .iter()
            .any(|req| req.to_string().starts_with("python >=3.13"));
        let has_python_constraint_run = run_requirements
            .iter()
            .any(|req| req.to_string().starts_with("python >=3.13"));

        assert!(
            has_python_constraint_host,
            "Host requirements should include 'python >=3.13', found: {host_requirements:?}"
        );
        assert!(
            has_python_constraint_run,
            "Run requirements should include 'python >=3.13', found: {run_requirements:?}"
        );
    }
}
