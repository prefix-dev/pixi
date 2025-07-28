use std::path::{Path, PathBuf};

use miette::IntoDiagnostic;

pub enum BackendType {
    PixiBuildPython,
}

impl BackendType {
    pub fn as_str(&self) -> &str {
        match self {
            BackendType::PixiBuildPython => "pixi-build-python",
        }
    }
}

/// Helper for creating fake source backend packages for testing
pub struct SourceBackendBuilder {
    package_name: String,
    backend_type: BackendType,
    version: String,
}

impl SourceBackendBuilder {
    /// Create a new source backend builder
    pub fn new(package_name: &str, backend_type: BackendType) -> Self {
        Self {
            package_name: package_name.to_string(),
            backend_type,
            version: "0.1.0".to_string(),
        }
    }

    /// Set the version (default is "0.1.0")
    pub fn with_version(mut self, version: &str) -> Self {
        self.version = version.to_string();
        self
    }

    fn build_python(&self, package_dir: &Path) -> miette::Result<()> {
        // Create pixi.toml
        let pixi_toml_content = format!(
            r#"
[workspace]
channels = ["https://prefix.dev/conda-forge"]
platforms = []
preview = ["pixi-build"]

[package]
name = "{package_name}"
version = "{version}"

[package.build]
backend = {{ name = "{backend_type}", version = "*" }}
channels = ["https://prefix.dev/pixi-build-backends", "https://prefix.dev/conda-forge"]

[package.host-dependencies]
hatchling = "*"

[package.run-dependencies]
pixi-build-api-version = "*"
"#,
            package_name = self.package_name,
            version = self.version,
            backend_type = self.backend_type.as_str()
        );
        std::fs::write(package_dir.join("pixi.toml"), pixi_toml_content).into_diagnostic()?;

        // Create Python package structure
        let python_package_dir = package_dir.join(&self.package_name.replace("-", "_"));
        std::fs::create_dir_all(&python_package_dir).into_diagnostic()?;

        // Create pyproject.toml
        let pyproject_toml_content = format!(
            r#"
[build-system]
build-backend = "hatchling.build"
requires = ["hatchling"]

[project]
name = "{package_name}"
version = "{version}"
description = "A fake pixi build backend for testing"
authors = [{{name = "Test Author", email = "test@example.com"}}]
requires-python = ">=3.8"

"#,
            package_name = self.package_name,
            version = self.version,
        );
        std::fs::write(package_dir.join("pyproject.toml"), pyproject_toml_content)
            .into_diagnostic()?;

        // Create __init__.py
        let init_py_content = format!(
            r#"
"""A fake pixi build backend for testing."""

def build_backend():
    """Fake function just for testing."""
    return "{package_name}"
"#,
            package_name = self.package_name,
        );
        std::fs::write(python_package_dir.join("__init__.py"), init_py_content)
            .into_diagnostic()?;
        Ok(())
    }

    /// Build the package structure in the given base directory and return the package path
    pub fn build(&self, base_dir: &Path) -> miette::Result<PathBuf> {
        let package_dir = base_dir.join("test-backend");
        std::fs::create_dir_all(&package_dir).into_diagnostic()?;

        match self.backend_type {
            BackendType::PixiBuildPython => self.build_python(&package_dir)?,
        };

        Ok(package_dir)
    }
}
