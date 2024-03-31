use pep508_rs::VersionOrUrl;
use rattler_conda_types::{NamelessMatchSpec, PackageName, ParseStrictness::Lenient, VersionSpec};
use serde::Deserialize;
use std::{path::PathBuf, str::FromStr};
use toml_edit;
use toml_edit::TomlError;

use super::{
    error::RequirementConversionError, python::PyPiPackageName, ProjectManifest, PyPiRequirement,
    SpecType,
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
        // Start by loading the data nested under "tool.pixi" as manifest,
        // and create a reference to the 'pyproject.toml' project table
        let mut manifest = item.tool.pixi.clone();
        let pyproject = item.project.as_ref().expect("project table should exist");

        // TODO: tool.pixi.project.name should be made optional or read from project.name
        // TODO: could copy across / convert some other optional fields if relevant

        // Add python as dependency based on the project.requires_python property (if any)
        let pythonspec = pyproject
            .requires_python
            .clone()
            .map(VersionOrUrl::VersionSpecifier);
        let target = manifest.default_feature_mut().targets.default_mut();
        target.add_dependency(
            PackageName::from_str("python").unwrap(),
            version_or_url_to_nameless_matchspec(&pythonspec).unwrap(),
            SpecType::Run,
        );

        // Add pyproject dependencies as pypi dependencies
        if let Some(deps) = pyproject.dependencies.clone() {
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::str::FromStr;

    use insta::assert_snapshot;

    use crate::{
        project::manifest::{python::PyPiPackageName, Manifest, PyPiRequirement},
        FeatureName,
    };

    const PYPROJECT_FULL: &str = r#"
        [project]
        name = "project"

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

    const PYPROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "flask-hello-world-pyproject"
        version = "0.1.0"
        description = "Example how to get started with flask in a pixi environment."
        license = "MIT OR Apache-2.0"
        homepage = "https://github.com/prefix/pixi"
        readme = "README.md"
        requires-python = ">=3.11"
        dependencies = ["flask==2.*"]

        [tool.pixi.project]
        name = "flask-hello-world-pyproject"
        channels = ["conda-forge"]
        platforms = ["linux-64"]

        [tool.pixi.tasks]
        start = "python -m flask run --port=5050"
        "#;

    #[test]
    fn test_build_manifest() {
        let _manifest = Manifest::from_str(Path::new("pyproject.toml"), PYPROJECT_FULL).unwrap();
    }

    #[test]
    fn test_add_pypi_dependency() {
        let mut manifest =
            Manifest::from_str(Path::new("pyproject.toml"), PYPROJECT_BOILERPLATE).unwrap();

        // Add numpy to pyproject
        let name = PyPiPackageName::from_str("numpy").unwrap();
        let requirement = PyPiRequirement::RawVersion(">=3.12".parse().unwrap());
        manifest
            .add_pypi_dependency(&name, &requirement, None)
            .unwrap();

        assert!(manifest
            .default_feature_mut()
            .targets
            .for_opt_target(None)
            .unwrap()
            .pypi_dependencies
            .as_ref()
            .unwrap()
            .get(&name)
            .is_some());

        assert_snapshot!(manifest.document.to_string());
    }

    #[test]
    fn test_remove_pypi_dependency() {
        let mut manifest =
            Manifest::from_str(Path::new("pyproject.toml"), PYPROJECT_BOILERPLATE).unwrap();

        // Remove flask from pyproject
        let name = PyPiPackageName::from_str("flask").unwrap();
        manifest
            .remove_pypi_dependency(&name, None, &FeatureName::Default)
            .unwrap();

        assert!(manifest
            .default_feature_mut()
            .targets
            .for_opt_target(None)
            .unwrap()
            .pypi_dependencies
            .as_ref()
            .unwrap()
            .get(&name)
            .is_none());

        assert_snapshot!(manifest.document.to_string());
    }
}
