use std::collections::BTreeSet;

use miette::Diagnostic;
use pixi_build_backend::generated_recipe::MetadataProvider;
use pyo3::{Py, PyAny, PyErr, Python, pyclass, pymethods};
use rattler_conda_types::{ParseVersionError, Version};
use std::str::FromStr;
use thiserror::Error;

/// Error type for Python metadata provider operations
#[derive(Debug, Error, Diagnostic)]
pub enum PyMetadataProviderError {
    #[error("Python metadata provider error: {0}")]
    Python(String),
    #[error("Failed to parse version: {0}")]
    ParseVersion(#[from] ParseVersionError),
}

impl From<PyErr> for PyMetadataProviderError {
    fn from(err: PyErr) -> Self {
        PyMetadataProviderError::Python(err.to_string())
    }
}

/// A Rust wrapper around a Python object that implements the MetadataProvider protocol
#[pyclass]
#[derive(Clone)]
pub struct PyMetadataProvider {
    inner: Py<PyAny>,
}

#[pymethods]
impl PyMetadataProvider {
    #[new]
    pub fn new(provider: Py<PyAny>) -> Self {
        Self { inner: provider }
    }
}

impl MetadataProvider for PyMetadataProvider {
    type Error = PyMetadataProviderError;

    fn name(&mut self) -> Result<Option<String>, Self::Error> {
        Python::attach(|py| {
            let result = self.inner.call_method0(py, "name")?;

            if result.is_none(py) {
                Ok(None)
            } else {
                let name: String = result.extract(py)?;
                Ok(Some(name))
            }
        })
    }

    fn version(&mut self) -> Result<Option<Version>, Self::Error> {
        Python::attach(|py| {
            let result = self.inner.call_method0(py, "version")?;

            if result.is_none(py) {
                Ok(None)
            } else {
                let version_str: String = result.extract(py)?;
                let version = Version::from_str(&version_str)?;
                Ok(Some(version))
            }
        })
    }

    fn homepage(&mut self) -> Result<Option<String>, Self::Error> {
        Python::attach(|py| {
            let result = self.inner.call_method0(py, "homepage")?;

            if result.is_none(py) {
                Ok(None)
            } else {
                let homepage: String = result.extract(py)?;
                Ok(Some(homepage))
            }
        })
    }

    fn license(&mut self) -> Result<Option<String>, Self::Error> {
        Python::attach(|py| {
            let result = self.inner.call_method0(py, "license")?;

            if result.is_none(py) {
                Ok(None)
            } else {
                let license: String = result.extract(py)?;
                Ok(Some(license))
            }
        })
    }

    fn license_files(&mut self) -> Result<Option<Vec<String>>, Self::Error> {
        Python::attach(|py| {
            let result = self.inner.call_method0(py, "license_files")?;

            if result.is_none(py) {
                Ok(None)
            } else {
                let license_file: String = result.extract(py)?;
                Ok(Some(vec![license_file]))
            }
        })
    }

    fn summary(&mut self) -> Result<Option<String>, Self::Error> {
        Python::attach(|py| {
            let result = self.inner.call_method0(py, "summary")?;

            if result.is_none(py) {
                Ok(None)
            } else {
                let summary: String = result.extract(py)?;
                Ok(Some(summary))
            }
        })
    }

    fn description(&mut self) -> Result<Option<String>, Self::Error> {
        Python::attach(|py| {
            let result = self.inner.call_method0(py, "description")?;

            if result.is_none(py) {
                Ok(None)
            } else {
                let description: String = result.extract(py)?;
                Ok(Some(description))
            }
        })
    }

    fn documentation(&mut self) -> Result<Option<String>, Self::Error> {
        Python::attach(|py| {
            let result = self.inner.call_method0(py, "documentation")?;

            if result.is_none(py) {
                Ok(None)
            } else {
                let documentation: String = result.extract(py)?;
                Ok(Some(documentation))
            }
        })
    }

    fn repository(&mut self) -> Result<Option<String>, Self::Error> {
        Python::attach(|py| {
            let result = self.inner.call_method0(py, "repository")?;

            if result.is_none(py) {
                Ok(None)
            } else {
                let repository: String = result.extract(py)?;
                Ok(Some(repository))
            }
        })
    }
}

/// Helper function to get input globs from a Python metadata provider
/// if it supports the input_globs method (optional)
pub fn get_input_globs_from_provider(provider: &Py<PyAny>) -> BTreeSet<String> {
    Python::attach(|py| {
        // Try to call input_globs method if it exists
        match provider.call_method0(py, "input_globs") {
            Ok(result) => {
                // Try to extract as a list of strings
                if let Ok(globs_list) = result.extract::<Vec<String>>(py) {
                    globs_list.into_iter().collect()
                } else {
                    BTreeSet::new()
                }
            }
            Err(_) => {
                // Method doesn't exist or failed, return empty set
                BTreeSet::new()
            }
        }
    })
}
