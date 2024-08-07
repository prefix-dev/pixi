use std::{collections::HashMap, fs, path::PathBuf, str::FromStr};

use indexmap::IndexMap;
use miette::{IntoDiagnostic, Report, WrapErr};
use pep440_rs::VersionSpecifiers;
use pep508_rs::Requirement;
use pyproject_toml::{self, Project};
use rattler_conda_types::{NamelessMatchSpec, PackageName, ParseStrictness::Lenient, VersionSpec};
use serde::Deserialize;
use toml_edit::DocumentMut;

use super::{
    error::{RequirementConversionError, TomlError},
    Feature, ParsedManifest, SpecType,
};
use crate::FeatureName;

#[derive(Deserialize, Debug, Clone)]
pub struct PyProjectManifest {
    #[serde(flatten)]
    inner: pyproject_toml::PyProjectToml,
    pub tool: Option<Tool>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Tool {
    pub pixi: Option<ParsedManifest>,
    pub poetry: Option<ToolPoetry>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ToolPoetry {
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub authors: Option<Vec<String>>,
}

impl std::ops::Deref for PyProjectManifest {
    type Target = pyproject_toml::PyProjectToml;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl PyProjectManifest {
    /// Parses a toml string into a PyProjectManifest
    pub fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        toml_edit::de::from_str(source).map_err(TomlError::from)
    }

    /// Parses a `pyproject.toml` file into a PyProjectManifest
    pub fn from_path(path: &PathBuf) -> Result<Self, Report> {
        let source = fs::read_to_string(path)
            .into_diagnostic()
            .wrap_err_with(|| format!("Failed to read file: {:?}", path))?;
        Self::from_toml_str(&source).into_diagnostic()
    }

    /// Ensures the `pyproject.toml` contains a `[tool.pixi]` table
    /// and project name is defined
    pub fn ensure_pixi(self, source: &str) -> Result<Self, TomlError> {
        // Make sure the `[tool.pixi]` table exist
        if !self.is_pixi() {
            return Err(TomlError::NoPixiTable);
        }

        // Make sure a 'name' is defined
        if self.name().is_none() {
            let document = source.parse::<DocumentMut>().map_err(TomlError::from)?;
            let span = document["tool"]["pixi"]["project"].span();
            return Err(TomlError::NoProjectName(span));
        }

        Ok(self)
    }

    fn tool(&self) -> Option<&Tool> {
        self.tool.as_ref()
    }

    pub fn project(&self) -> Option<&Project> {
        self.project.as_ref()
    }

    pub fn poetry(&self) -> Option<&ToolPoetry> {
        self.tool().and_then(|t| t.poetry.as_ref())
    }

    fn pixi(&self) -> Option<&ParsedManifest> {
        self.tool().and_then(|t| t.pixi.as_ref())
    }

    /// Checks whether a `pyproject.toml` is valid for use with pixi by
    /// checking it contains a `[tool.pixi]` table.
    pub fn is_pixi(&self) -> bool {
        self.pixi().is_some()
    }

    /// Returns the project name from, in order of priority
    ///  - the `[tool.pixi.project]` table
    ///  - the `[project]` table
    ///  - the `[tool.poetry]` table
    pub fn name(&self) -> Option<String> {
        if let Some(pixi_name) = self.pixi().and_then(|p| p.project.name.as_ref()) {
            return Some(pixi_name.clone());
        }
        if let Some(pyproject) = &self.project {
            return Some(pyproject.name.clone());
        }
        if let Some(poetry_name) = self.poetry().and_then(|p| p.name.as_ref()) {
            return Some(poetry_name.clone());
        }
        None
    }

    /// Returns the project description from, in order of priority
    ///  - the `[tool.pixi.project]` table
    ///  - the `[project]` table
    ///  - the `[tool.poetry]` table
    fn description(&self) -> Option<String> {
        if let Some(pixi_description) = self.pixi().and_then(|p| p.project.description.as_ref()) {
            return Some(pixi_description.to_string());
        }
        if let Some(pyproject_description) =
            self.project.as_ref().and_then(|p| p.description.as_ref())
        {
            return Some(pyproject_description.to_string());
        }
        if let Some(poetry_description) = self.poetry().and_then(|p| p.description.as_ref()) {
            return Some(poetry_description.clone());
        }
        None
    }

    /// Returns the project version from, in order of priority
    ///  - the `[tool.pixi.project]` table
    ///  - the `[project]` table
    ///  - the `[tool.poetry]` table
    fn version(&self) -> Option<String> {
        if let Some(pixi_version) = self.pixi().and_then(|p| p.project.version.as_ref()) {
            return Some(pixi_version.to_string());
        }
        if let Some(pyproject_version) = self.project.as_ref().and_then(|p| p.version.as_ref()) {
            return Some(pyproject_version.to_string());
        }
        if let Some(poetry_version) = self.poetry().and_then(|p| p.version.as_ref()) {
            return Some(poetry_version.clone());
        }
        None
    }

    /// Returns the project authors from, in order of priority
    ///  - the `[tool.pixi.project]` table
    ///  - the `[project]` table
    ///  - the `[tool.poetry]` table
    fn authors(&self) -> Option<Vec<String>> {
        if let Some(pixi_authors) = self.pixi().and_then(|p| p.project.authors.as_ref()) {
            return Some(pixi_authors.clone());
        }
        if let Some(pyproject_authors) = self.project.as_ref().and_then(|p| p.authors.as_ref()) {
            return Some(
                pyproject_authors
                    .iter()
                    .filter_map(|contact| match (&contact.name, &contact.email) {
                        (Some(name), Some(email)) => Some(format!("{} <{}>", name, email)),
                        (Some(name), None) => Some(name.clone()),
                        (None, Some(email)) => Some(email.clone()),
                        (None, None) => None,
                    })
                    .collect(),
            );
        }
        if let Some(poetry_authors) = self.poetry().and_then(|p| p.authors.as_ref()) {
            return Some(poetry_authors.clone());
        }
        None
    }

    /// Returns the project name as PEP508 name
    fn package_name(&self) -> Option<pep508_rs::PackageName> {
        self.name()
            .and_then(|n| pep508_rs::PackageName::new(n).ok())
    }

    /// Returns optional dependencies from the `[project.optional-dependencies]` table
    fn optional_dependencies(&self) -> Option<IndexMap<String, Vec<Requirement>>> {
        self.project().and_then(|p| p.optional_dependencies.clone())
    }

    /// Builds a list of pixi environments from pyproject groups of extra
    /// dependencies:
    ///  - one environment is created per group of extra, with the same name as
    ///    the group of extra
    ///  - each environment includes the feature of the same name as the group
    ///    of extra
    ///  - it will also include other features inferred from any self references
    ///    to other groups of extras
    pub fn environments_from_extras(&self) -> HashMap<String, Vec<String>> {
        let mut environments = HashMap::new();
        if let Some(extras) = self.optional_dependencies() {
            let pname = self.package_name();
            for (extra, reqs) in extras {
                let mut features = vec![extra.to_string()];
                // Add any references to other groups of extra dependencies
                for req in reqs.iter() {
                    if pname.as_ref() == Some(&req.name) {
                        for extra in &req.extras {
                            features.push(extra.to_string())
                        }
                    }
                }
                // Environments can only contain number, strings and dashes
                environments.insert(extra.replace('_', "-").clone(), features);
            }
        }
        environments
    }
}

impl From<PyProjectManifest> for ParsedManifest {
    fn from(item: PyProjectManifest) -> Self {
        // Load the data nested under '[tool.pixi]' as pixi manifest
        let mut manifest = item
            .pixi()
            .expect("The [tool.pixi] table should exist")
            .clone();

        // Set pixi project name, version, description and authors (if they are not set)
        // with the ones from the `[project]` or `[tool.poetry]` tables of the `pyproject.toml`.
        manifest.project.name = item.name();
        manifest.project.description = item.description();
        manifest.project.version = item.version().and_then(|v| v.parse().ok());
        manifest.project.authors = item.authors();

        // TODO:  would be nice to add license, license-file, readme, homepage, repository, documentation,
        // regarding the above, the types are a bit different than we expect, so the conversion is not straightforward
        // we could change these types or we can convert. Let's decide when we make it.
        // etc.

        // Add python as dependency based on the `project.requires_python` property
        let python_spec = item.project().and_then(|p| p.requires_python.clone());

        let target = manifest.default_feature_mut().targets.default_mut();
        let python = PackageName::from_str("python").unwrap();
        // If the target doesn't have any python dependency, we add it from the
        // `requires-python`
        if !target.has_dependency(&python, Some(SpecType::Run), None) {
            target.add_dependency(
                &python,
                &version_or_url_to_nameless_matchspec(&python_spec).unwrap(),
                SpecType::Run,
            );
        } else if let Some(_spec) = python_spec {
            if target.has_dependency(&python, Some(SpecType::Run), None) {
                // TODO: implement some comparison or spec merging logic here
                tracing::info!(
                    "Overriding the requires-python with the one defined in pixi dependencies"
                )
            }
        }

        // Add pyproject dependencies as pypi dependencies
        if let Some(deps) = item.project().and_then(|p| p.dependencies.clone()) {
            for requirement in deps.iter() {
                target.add_pypi_dependency(requirement, None);
            }
        }

        // For each extra group, create a feature of the same name if it does not exist,
        // and add pypi dependencies from project.optional-dependencies,
        // filtering out self-references
        if let Some(extras) = item.optional_dependencies() {
            let project_name = item.package_name();
            for (extra, reqs) in extras {
                let feature_name = FeatureName::Named(extra.to_string());
                let target = manifest
                    .features
                    .entry(feature_name.clone())
                    .or_insert_with(move || Feature::new(feature_name))
                    .targets
                    .default_mut();
                for requirement in reqs.iter() {
                    // filter out any self references in groups of extra dependencies
                    if project_name.as_ref() != Some(&requirement.name) {
                        target.add_pypi_dependency(requirement, None);
                    }
                }
            }
        }

        manifest
    }
}

/// Try to return a NamelessMatchSpec from a pep508_rs::VersionOrUrl
/// This will only work if it is not URL and the VersionSpecifier can
/// successfully be interpreted as a NamelessMatchSpec.version
fn version_or_url_to_nameless_matchspec(
    version: &Option<VersionSpecifiers>,
) -> Result<NamelessMatchSpec, RequirementConversionError> {
    match version {
        // TODO: avoid going through string representation for conversion
        Some(v) => {
            let version_string = v.to_string();
            // Double equals works a bit different in conda vs. python
            let version_string = version_string.strip_prefix("==").unwrap_or(&version_string);

            Ok(NamelessMatchSpec::from_str(version_string, Lenient)?)
        }
        None => Ok(NamelessMatchSpec {
            version: Some(VersionSpec::Any),
            ..Default::default()
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, str::FromStr};

    use insta::assert_snapshot;
    use pep440_rs::VersionSpecifiers;
    use rattler_conda_types::{ParseStrictness, VersionSpec};

    use crate::{
        manifest::Manifest, pypi::PyPiPackageName, DependencyOverwriteBehavior, FeatureName,
    };

    const PYPROJECT_FULL: &str = r#"
        [project]
        name = "project"
        version = "0.1.0"
        description = "A project"
        authors = [
            { name = "Author", email = "author@bla.com" }
        ]

        [tool.pixi.project]
        channels = ["stable"]
        platforms = ["linux-64", "win-64", "osx-64", "osx-arm64"]
        license = "MIT"
        license-file = "../../LICENSE"
        readme = "../../README.md"
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
        test = { cmd = "pytest", cwd = "tests", depends-on = ["build"] }
        test2 = { cmd = "pytest", cwd = "tests"}
        test3 = { cmd = "pytest", depends-on = ["test2"] }
        test5 = { cmd = "pytest" }
        test6 = { depends-on = ["test5"] }

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
        test = { cmd = "pytest", cwd = "tests", depends-on = ["build"] }
        test2 = { cmd = "pytest", cwd = "tests"}
        test3 = { cmd = "pytest", depends-on = ["test2"] }
        test5 = { cmd = "pytest" }
        test6 = { depends-on = ["test5"] }

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
        let requirement = pep508_rs::Requirement::from_str("numpy>=3.12").unwrap();
        manifest
            .add_pypi_dependency(
                &requirement,
                &[],
                &FeatureName::Default,
                None,
                DependencyOverwriteBehavior::Overwrite,
            )
            .unwrap();

        assert!(manifest
            .default_feature_mut()
            .targets
            .for_opt_target(None)
            .unwrap()
            .pypi_dependencies
            .as_ref()
            .unwrap()
            .get(&PyPiPackageName::from_normalized(requirement.name.clone()))
            .is_some());

        // Add numpy to feature in pyproject
        let requirement = pep508_rs::Requirement::from_str("pytest>=3.12").unwrap();
        manifest
            .add_pypi_dependency(
                &requirement,
                &[],
                &FeatureName::Named("test".to_string()),
                None,
                DependencyOverwriteBehavior::Overwrite,
            )
            .unwrap();
        assert!(manifest
            .feature(&FeatureName::Named("test".to_string()))
            .unwrap()
            .targets
            .for_opt_target(None)
            .unwrap()
            .pypi_dependencies
            .as_ref()
            .unwrap()
            .get(&PyPiPackageName::from_normalized(requirement.name.clone()))
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
            .remove_pypi_dependency(&name, &[], &FeatureName::Default)
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

    #[test]
    fn test_version_url_to_matchspec() {
        fn cmp(v1: &str, v2: &str) {
            let v = VersionSpecifiers::from_str(v1).unwrap();
            let matchspec = super::version_or_url_to_nameless_matchspec(&Some(v)).unwrap();
            let vspec = VersionSpec::from_str(v2, ParseStrictness::Strict).unwrap();
            assert_eq!(matchspec.version, Some(vspec));
        }

        // Check that we remove leading `==` for the conda version spec
        cmp("==3.12", "3.12");
        cmp("==3.12.*", "3.12.*");
        // rest should work just fine
        cmp(">=3.12", ">=3.12");
        cmp(">=3.10,<3.12", ">=3.10,<3.12");
        cmp("~=3.12", "~=3.12");
    }
}
