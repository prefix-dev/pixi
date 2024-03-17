use pep508_rs::{Requirement, VersionOrUrl};
use rattler_conda_types::{NamelessMatchSpec, PackageName, ParseStrictness::Lenient, VersionSpec};
use serde::Deserialize;
use std::collections::HashMap;
use std::str::FromStr;
use tokio::runtime::Handle;
use toml_edit;
use toml_edit::TomlError;

use crate::pypi_name_mapping::conda_pypi_name_mapping;

use super::{
    error::RequirementConversionError, python::PyPiPackageName, ProjectManifest, SpecType,
};

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
        // TODO: Skip processing if in a dedicated "tool.pixi" section to allow manual overrides?
        let target = manifest
            .default_feature_mut()
            .targets
            .for_opt_target_or_default_mut(None);
        for requirement in requirements {
            // Skip requirement if it is already a Pypi Dependency of the default feature of the default target
            match PyPiPackageName::from_str(requirement.name.to_string().as_str()) {
                Ok(pypi_name) => {
                    if target
                        .pypi_dependencies
                        .as_ref()
                        .map(|d| d.contains_key(&pypi_name))
                        .unwrap_or(false)
                    {
                        continue;
                    }
                }
                _ => {
                    // When the conversion from Requirement.name to PyPiPackageName fails
                    tracing::debug!("Unable to interpret {:?}", requirement);
                }
            }
            match req_to_conda_name(&requirement) {
                Ok(name) => {
                    let rundeps = target.dependencies.entry(SpecType::Run).or_default();
                    // Skip requirement if it is already a Run Dependency of the default feature of the default target
                    if rundeps.contains_key(&name) {
                        continue;
                    }
                    // Otherwise add it as a Run Dependency of the default feature of the default target
                    match req_to_nmspec(&requirement) {
                        Ok(spec) => {
                            rundeps.insert(name, spec);
                        }
                        _ => {
                            // When the conversion from VersionSpecifiers to NamelessMatchSpec fails
                            tracing::debug!("Unable to interpret {:?}", requirement);
                        }
                    }
                }
                _ => {
                    // When the name conversion fails
                    tracing::debug!("Unable to interpret {:?}", requirement);
                }
            }
        }

        // For each extra group, create a feature of the same name if it does not exist,
        // add dependencies and create corresponding environments if they do not exist
        // TODO: Add solve groups as well?
        // TODO: Deal with self referencing extras?

        manifest
    }
}

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
