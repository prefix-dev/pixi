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
    error::RequirementConversionError, python::PyPiPackageName, ProjectManifest, PyPiRequirement,
    SpecType, Target,
};

#[derive(Deserialize, Debug, Clone)]
pub struct PyProjectManifest {
    #[serde(flatten)]
    inner: pyproject_toml::PyProjectToml,
    tool: Tool,
}

#[derive(Deserialize, Debug, Clone)]
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
        let target = manifest.default_feature_mut().targets.default_mut();
        target.try_install_requirements_as_conda(vec![python_req]);

        // add pyproject dependencies as pypi dependencies
        if let Some(deps) = item
            .project
            .as_ref()
            .and_then(|p| p.dependencies.as_ref())
            .cloned()
        {
            for d in deps.into_iter() {
                target.add_pypi_dependency(
                    PyPiPackageName::from_normalized(d.name.clone()),
                    PyPiRequirement::from(d),
                )
            }
        }

        // For each extra group, create a feature of the same name if it does not exist,
        // add dependencies and create corresponding environments if they do not exist
        // TODO: Add solve groups as well?
        // TODO: Deal with self referencing extras?

        manifest
    }
}

impl Target {
    /// Install a vec of Pypi requirements as conda dependencies
    /// unless they are specified in "tool.pixi.pypi-dependencies" or "tool.pixi.dependencies"
    fn try_install_requirements_as_conda(&mut self, requirements: Vec<Requirement>) {
        for requirement in requirements {
            // Skip requirement if it is already a Pypi Dependency of the target
            if self.has_pypi_dependency(requirement.name.to_string().as_str()) {
                continue;
            }

            // Convert the pypi name to a conda package name
            // TODO: create a dedicated "tool.pixi" section to allow manual overrides?
            let conda_dep = req_to_conda_name(&requirement);
            if conda_dep.is_err() {
                tracing::warn!("Unable to get conda package name for {:?}", requirement);
                continue;
            }

            // Skip requirement if it is already a Dependency of the target
            if self.has_dependency(conda_dep.as_ref().unwrap().as_normalized(), None) {
                continue;
            }

            // Convert requirement to a Spec.
            let spec = requirement_to_nameless_matchspec(&requirement);
            if spec.is_err() {
                tracing::warn!("Unable to build conda spec for {:?}", requirement);
                continue;
            }

            // Add conda dependency
            self.add_dependency(conda_dep.unwrap(), spec.unwrap(), SpecType::Run);
        }
    }
}

/// Return the conda rattler_conda_types::PackageName corresponding to a pep508_rs::Requirement
/// If the pep508_rs::Requirement name is not present in the conda to pypi mapping
/// then returns PackageName from the pypi name
fn req_to_conda_name(requirement: &Requirement) -> Result<PackageName, RequirementConversionError> {
    let pypi_name = requirement.name.to_string();
    let handle = Handle::current();
    let map = futures::executor::block_on(async move {
        handle
            .spawn(async move { conda_pypi_name_mapping().await })
            .await
            .unwrap()
    })
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
fn version_or_url_to_nameless_matchspec(
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
fn requirement_to_nameless_matchspec(
    requirement: &Requirement,
) -> Result<NamelessMatchSpec, RequirementConversionError> {
    match requirement {
        Requirement {
            extras,
            version_or_url,
            marker: None,
            ..
        } if extras.is_empty() => version_or_url_to_nameless_matchspec(version_or_url),
        _ => Err(RequirementConversionError::Unimplemented),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::project::manifest::{Manifest, ManifestKind};

    const PYPROJECT_FULL: &str = r#"
        [tool.pixi.project]
        name = "project"
        version = "0.1.0"
        description = "A project"
        authors = ["Author <author@bla.com>"]
        channels = ["stable"]
        platforms = ["linux-64", "win-64", "osx-64", "osx-arm64"]
        license = "MIT"
        license-file = "LICENSE"
        readme = "README.md"
        homepage = "https://project.com"
        repository = "https://github.com/author/project"
        documentation = "https://docs.project.com"

        [tool.pixi.dependencies]
        test = "bla"
        test1 = "bli"
        pytorch-cpu = { version = "~=1.1", channel = "pytorch" }
        package1 = { version = ">=1.2.3", build="py34_0" }


        [tool.pixi.pypi-dependencies]
        testpypi = "*"
        testpypi1 = "*"
        requests = {version = ">= 2.8.1, ==2.8.*", extras=["security", "tests"]} # Using the map allows the user to add `extras`

        [tool.pixi.host-dependencies]
        test = "bla"
        test1 = "bli"
        pytorch-cpu = { version = "~=1.1", channel = "pytorch" }
        package1 = { version = ">=1.2.3", build="py34_0" }

        [tool.pixi.build-dependencies]
        test = "*"
        test1 = "*"
        pytorch-cpu = { version = "~=1.1", channel = "pytorch" }
        package1 = { version = ">=1.2.3", build="py34_0" }

        [tool.pixi.tasks]
        build = "conda build ."
        test = { cmd = "pytest", cwd = "tests", depends_on = ["build"] }
        test2 = { cmd = "pytest", cwd = "tests"}
        test3 = { cmd = "pytest", depends_on = ["test2"] }
        test5 = { cmd = "pytest" }
        test6 = { depends_on = ["test5"] }

        [tool.pixi.system-requirements]
        linux = "5.10"
        libc = { family="glibc", version="2.17" }
        cuda = "10.1"

        [tool.pixi.feature.test.dependencies]
        test = "bla"

        [tool.pixi.feature.test2.dependencies]
        test = "bla"

        [tool.pixi.environments]
        test = {features = ["test"], solve-group = "test"}
        prod = {features = ["test2"], solve-group = "test"}

        [tool.pixi.activation]
        scripts = ["activate.sh", "deactivate.sh"]

        [tool.pixi.target.win-64.activation]
        scripts = ["env_setup.bat"]

        [tool.pixi.target.linux-64.dependencies]
        test = "bla"
        test1 = "bli"
        pytorch-cpu = { version = "~=1.1", channel = "pytorch" }
        package1 = { version = ">=1.2.3", build="py34_0" }


        [tool.pixi.target.osx-arm64.pypi-dependencies]
        testpypi = "*"
        testpypi1 = "*"
        requests = {version = ">= 2.8.1, ==2.8.*", extras=["security", "tests"]} # Using the map allows the user to add `extras`

        [tool.pixi.target.osx-64.host-dependencies]
        test = "bla"
        test1 = "bli"
        pytorch-cpu = { version = "~=1.1", channel = "pytorch" }
        package1 = { version = ">=1.2.3", build="py34_0" }

        [tool.pixi.target.linux-64.build-dependencies]
        test = "bla"
        test1 = "bli"
        pytorch-cpu = { version = "~=1.1", channel = "pytorch" }
        package1 = { version = ">=1.2.3", build="py34_0" }

        [tool.pixi.target.linux-64.tasks]
        build = "conda build ."
        test = { cmd = "pytest", cwd = "tests", depends_on = ["build"] }
        test2 = { cmd = "pytest", cwd = "tests"}
        test3 = { cmd = "pytest", depends_on = ["test2"] }
        test5 = { cmd = "pytest" }
        test6 = { depends_on = ["test5"] }

        [tool.pixi.feature.test.target.linux-64.dependencies]
        test = "bla"

        [tool.pixi.feature.cuda]
        activation = {scripts = ["cuda_activation.sh"]}
        channels = ["nvidia"] # Results in:  ["nvidia", "conda-forge"] when the default is `conda-forge`
        dependencies = {cuda = "x.y.z", cudnn = "12.0"}
        pypi-dependencies = {torch = "==1.9.0"}
        platforms = ["linux-64", "osx-arm64"]
        system-requirements = {cuda = "12"}
        tasks = { warmup = "python warmup.py" }
        target.osx-arm64 = {dependencies = {mlx = "x.y.z"}}

        [tool.pixi.feature.cuda2.activation]
        scripts = ["cuda_activation.sh"]

        [tool.pixi.feature.cuda2.dependencies]
        cuda = "x.y.z"
        cudnn = "12.0"

        [tool.pixi.feature.cuda2.pypi-dependencies]
        torch = "==1.9.0"

        [tool.pixi.feature.cuda2.system-requirements]
        cuda = "12"

        [tool.pixi.feature.cuda2.tasks]
        warmup = "python warmup.py"

        [tool.pixi.feature.cuda2.target.osx-arm64.dependencies]
        mlx = "x.y.z"

        # Channels and Platforms are not available as separate tables as they are implemented as lists
        [tool.pixi.feature.cuda2]
        channels = ["nvidia"]
        platforms = ["linux-64", "osx-arm64"]
        "#;

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_build_manifest() {
        let _manifest =
            Manifest::from_str(Path::new(""), PYPROJECT_FULL, ManifestKind::Pyproject).unwrap();
    }
}
