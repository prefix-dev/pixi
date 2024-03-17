use pep508_rs::{Requirement, VersionOrUrl};
use rattler_conda_types::{NamelessMatchSpec, PackageName, ParseStrictness::Lenient, VersionSpec};
use serde::Deserialize;
use std::collections::HashMap;
use std::str::FromStr;
use tokio::runtime::Handle;
use toml_edit;
use toml_edit::TomlError;

use crate::pypi_name_mapping::conda_pypi_name_mapping;

use super::{error::RequirementConversionError, ProjectManifest, SpecType};

#[derive(Deserialize, Debug, Clone)]
pub struct PyProjectManifest {
    #[serde(flatten)]
    inner: pyproject_toml::PyProjectToml,
    tool: Tool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
struct Tool {
    pixi: ProjectManifest,
}

impl std::ops::Deref for PyProjectManifest {
    type Target = pyproject_toml::PyProjectToml;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl PyProjectManifest {
    /// Parses a toml string into a pyproject manifest.
    pub fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        toml_edit::de::from_str(source).map_err(TomlError::from)
    }
}

impl From<PyProjectManifest> for ProjectManifest {
    fn from(item: PyProjectManifest) -> Self {
        // Start by loading the data nested under "tool.pixi"
        let mut manifest = item.tool.pixi.clone();

        // TODO: tool.pixi.project.name should be made optional or read from project.name
        // TODO: could copy across / convert some other optional fields if relevant

        // Gather pyproject dependencies
        let mut requirements = item
            .project
            .as_ref()
            .and_then(|p| p.dependencies.as_ref())
            .cloned()
            .unwrap_or_else(Vec::new);

        // Add python as dependency based on the project.requires_python property (if any)
        let pythonspec = item
            .project
            .as_ref()
            .and_then(|p| p.requires_python.as_ref())
            .map(|v| VersionOrUrl::VersionSpecifier(v.clone()));
        let python_req = Requirement {
            name: pep508_rs::PackageName::from_str("python").unwrap(),
            version_or_url: pythonspec,
            extras: Vec::new(),
            marker: None,
        };
        requirements.push(python_req);

        // Add project.dependencies python dependencies as conda dependencies for the default feature
        // unless they are specified in "tool.pixi.pypi-dependencies" or "tool.pixi.dependencies"
        let target = manifest.default_feature_mut().targets.default_mut();
        for requirement in requirements {
            // Skip requirement if it is already a Pypi Dependency of the default target
            if target.has_pypi_dependency(requirement.name.to_string().as_str()) {
                continue;
            }

            // Convert the pypi name to a conda package name
            // TODO: create a dedicated "tool.pixi" section to allow manual overrides?
            let conda_dep = req_to_conda_name(&requirement);
            if conda_dep.is_err() {
                tracing::warn!("Unable to get conda package name for {:?}", requirement);
                continue;
            }

            // Skip requirement if it is already a Dependency of the default target
            if target.has_dependency(conda_dep.as_ref().unwrap().as_normalized(), None) {
                continue;
            }

            // Convert requirement to a Spec.
            let spec = req_to_nmspec(&requirement);
            if spec.is_err() {
                tracing::warn!("Unable to build conda spec for {:?}", requirement);
                continue;
            }

            // Add conda dependency
            target.add_dependency(conda_dep.unwrap(), spec.unwrap(), SpecType::Run);
        }

        // For each extra group, create a feature of the same name if it does not exist,
        // add dependencies and create corresponding environments if they do not exist
        // TODO: Add solve groups as well?
        // TODO: Deal with self referencing extras?

        manifest
    }
}

/// Return the conda rattler_conda_types::PackageName corresponding to a pep508_rs::Requirement
/// If the pep508_rs::Requirement name is not present in the conda to pypi mapping
/// then returns PackageName from the pypi name
fn req_to_conda_name(requirement: &Requirement) -> Result<PackageName, RequirementConversionError> {
    let pypi_name = requirement.name.to_string();
    let handle = Handle::current();
    let _guard = handle.enter();
    let map = futures::executor::block_on(conda_pypi_name_mapping())
        .map_err(|_| RequirementConversionError::MappingError)?;
    let pypi_to_conda: HashMap<String, String> =
        map.iter().map(|(k, v)| (v.clone(), k.clone())).collect();
    let name: PackageName = pypi_to_conda
        .get(&pypi_name)
        .unwrap_or(&pypi_name)
        .try_into()?;
    Ok(name)
}

/// Try to return a NamelessMatchSpec from a pep508_rs::VersionOrUrl
/// This will only work if it is not URL and the VersionSpecifier can successfully
/// be interpreted as a NamelessMatchSpec.version
fn version_or_url_to_nmspec(
    version: &Option<VersionOrUrl>,
) -> Result<NamelessMatchSpec, RequirementConversionError> {
    match version {
        // TODO: avoid going through string representation for conversion
        Some(VersionOrUrl::VersionSpecifier(v)) => Ok(NamelessMatchSpec::from_str(
            v.to_string().as_str(),
            Lenient,
        )?),
        Some(VersionOrUrl::Url(_)) => Err(RequirementConversionError::Unimplemented),
        None => Ok(NamelessMatchSpec {
            version: Some(VersionSpec::Any),
            ..Default::default()
        }),
    }
}

/// Try to return a NamelessMatchSpec from a pep508_rs::Requirement
/// This will only work if the Requirement has no extra not marker
/// and does not point to a URL
fn req_to_nmspec(
    requirement: &Requirement,
) -> Result<NamelessMatchSpec, RequirementConversionError> {
    match requirement {
        Requirement {
            extras,
            version_or_url,
            marker: None,
            ..
        } if extras.is_empty() => version_or_url_to_nmspec(version_or_url),
        _ => Err(RequirementConversionError::Unimplemented),
    }
}
