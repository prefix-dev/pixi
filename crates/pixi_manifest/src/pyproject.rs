use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
};

use indexmap::IndexMap;
use miette::{Diagnostic, IntoDiagnostic, Report, WrapErr};
use pep440_rs::{Version, VersionSpecifiers};
use pep508_rs::Requirement;
use pixi_spec::PixiSpec;
use pyproject_toml::{self, has_recursion::RecursionResolutionError, Contact};
use rattler_conda_types::{PackageName, ParseStrictness::Lenient, VersionSpec};
use thiserror::Error;
use toml_span::Spanned;

use super::{
    error::{RequirementConversionError, TomlError},
    DependencyOverwriteBehavior, Feature, SpecType, WorkspaceManifest,
};
use crate::{
    error::{DependencyError, GenericError},
    manifests::PackageManifest,
    toml::{
        pyproject::{TomlContact, TomlDependencyGroups, TomlOptionalDependencies, TomlProject},
        ExternalPackageProperties, ExternalWorkspaceProperties, FromTomlStr, PyProjectToml,
        TomlManifest,
    },
    FeatureName, ManifestKind, Warning,
};

#[derive(Debug)]
pub struct PyProjectManifest {
    pub project: PyProjectToml,
    pub tool: Option<Tool>,
}

#[derive(Debug)]
pub struct Tool {
    pub pixi: Option<TomlManifest>,
    pub poetry: Option<ToolPoetry>,
}

#[derive(Default, Debug)]
pub struct ToolPoetry {
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub authors: Option<Vec<String>>,
}

impl PyProjectManifest {
    /// Parses a `pyproject.toml` file into a PyProjectManifest
    pub fn from_path(path: &PathBuf) -> Result<Self, Report> {
        let source = fs_err::read_to_string(path)
            .into_diagnostic()
            .wrap_err_with(|| format!("Failed to read file: {:?}", path))?;
        Self::from_toml_str(&source).into_diagnostic()
    }

    /// Ensures the `pyproject.toml` contains a `[tool.pixi]` table
    /// and project name is defined
    pub fn ensure_pixi(self) -> Result<Self, TomlError> {
        // Make sure the `[tool.pixi]` table exist
        if !self.has_pixi_table() {
            return Err(TomlError::NoPixiTable(ManifestKind::Pyproject, None));
        }

        // Make sure a 'name' is defined
        if self.name().is_none() {
            let span = self
                .pixi_manifest()
                .and_then(|manifest| manifest.workspace.as_ref()?.span());
            return Err(TomlError::MissingField("name".into(), span));
        }

        Ok(self)
    }

    /// Returns the project name from, in order of priority
    ///  - the `[tool.pixi.project]` table
    ///  - the `[project]` table
    ///  - the `[tool.poetry]` table
    pub fn name(&self) -> Option<&str> {
        if let Some(pixi_name) = self
            .pixi_manifest()
            .and_then(|p| p.workspace.as_ref()?.value.name.as_deref())
        {
            return Some(pixi_name);
        }
        if let Some(pyproject) = &self.project.project {
            return Some(pyproject.name.value.as_str());
        }
        if let Some(poetry_name) = self.poetry().and_then(|p| p.name.as_ref()) {
            return Some(poetry_name.as_str());
        }
        None
    }

    fn tool(&self) -> Option<&Tool> {
        self.tool.as_ref()
    }

    /// Returns a reference to the poetry section if it exists.
    pub fn poetry(&self) -> Option<&ToolPoetry> {
        self.tool().and_then(|t| t.poetry.as_ref())
    }

    /// Returns a reference to the pixi section if it exists.
    fn pixi_manifest(&self) -> Option<&TomlManifest> {
        self.tool().and_then(|t| t.pixi.as_ref())
    }

    /// Checks whether a `pyproject.toml` is valid for use with pixi by
    /// checking it contains a `[tool.pixi]` table.
    pub fn has_pixi_table(&self) -> bool {
        self.pixi_manifest().is_some()
    }

    /// Returns optional dependencies from the `[project.optional-dependencies]`
    /// table
    fn optional_dependencies(
        &self,
        project_name: Option<&str>,
    ) -> Option<Result<IndexMap<String, Vec<Requirement>>, RecursionResolutionError>> {
        let project = self.project.project.as_ref()?;
        let optional_dependencies = project.optional_dependencies.as_ref()?;
        Some(optional_dependencies.value.0.resolve(project_name))
    }

    /// Returns dependency groups from the `[dependency-groups]` table
    fn dependency_groups(
        &self,
    ) -> Option<Result<IndexMap<String, Vec<Requirement>>, RecursionResolutionError>> {
        let dg = self.project.dependency_groups.as_ref()?;
        Some(dg.value.0.resolve())
    }

    /// Builds a list of pixi environments from pyproject groups of optional
    /// dependencies and/or dependency groups:
    ///  - one environment is created per group with the same name
    ///  - each environment includes the feature of the same name
    pub fn environments_from_dependency_groups(
        &self,
    ) -> Result<HashMap<String, Vec<String>>, RecursionResolutionError> {
        let mut environments = HashMap::new();

        let groups = self
            // no need to pass project name to resolve recursions properly here,
            // as only group names are used downstream
            .optional_dependencies(None)
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .chain(self.dependency_groups().transpose()?.unwrap_or_default());

        for (group, _) in groups {
            let normalised = group.replace('_', "-");
            environments
                .entry(normalised.clone())
                .or_insert_with(|| vec![group]);
        }

        Ok(environments)
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum PyProjectToManifestError {
    #[error("Unsupported pep508 requirement: '{0}'")]
    DependencyError(Requirement, #[source] DependencyError),
    #[error(transparent)]
    DependencyGroupError(#[from] RecursionResolutionError),
    #[error(transparent)]
    TomlError(#[from] TomlError),
}

#[derive(Default)]
pub struct PyProjectFields {
    pub name: Option<Spanned<String>>,
    pub description: Option<Spanned<String>>,
    pub version: Option<Spanned<Version>>,
    pub authors: Option<Vec<Spanned<TomlContact>>>,
    pub requires_python: Option<Spanned<VersionSpecifiers>>,
    pub dependencies: Option<Vec<Spanned<Requirement>>>,
    pub optional_dependencies: Option<Spanned<TomlOptionalDependencies>>,
}

impl From<TomlProject> for PyProjectFields {
    fn from(project: TomlProject) -> Self {
        Self {
            name: Some(project.name),
            description: project.description,
            version: project.version,
            authors: project.authors,
            requires_python: project.requires_python,
            dependencies: project.dependencies,
            optional_dependencies: project.optional_dependencies,
        }
    }
}

impl PyProjectManifest {
    /// Returns true if the pyproject.toml file also contains a pixi workspace.
    pub fn has_pixi_workspace(&self) -> bool {
        self.tool()
            .and_then(|t| t.pixi.as_ref())
            .is_some_and(TomlManifest::has_workspace)
    }

    /// Assume that the manifest is a workspace manifest and convert it as such.
    ///
    /// If the manifest also contains a package section that will be converted
    /// as well.
    pub fn into_package_manifest(
        self,
        workspace: &WorkspaceManifest,
        root_directory: Option<&Path>,
    ) -> Result<(PackageManifest, Vec<Warning>), TomlError> {
        // Load the data nested under '[tool.pixi]' as pixi manifest
        let Some(Tool {
            pixi: Some(pixi),
            poetry,
        }) = self.tool
        else {
            return Err(TomlError::MissingField("tool.pixi".into(), None));
        };

        // Extract some of the values we are interested in from the poetry table.
        let poetry = poetry.unwrap_or_default();

        // Extract the values we are interested in from the pyproject.toml
        let project = self
            .project
            .project
            .map(PyProjectFields::from)
            .unwrap_or_default();

        // TODO:  would be nice to add license, license-file, readme, homepage,
        // repository, documentation, regarding the above, the types are a bit
        // different than we expect, so the conversion is not straightforward we
        // could change these types or we can convert. Let's decide when we make it.
        // etc.
        pixi.into_package_manifest(
            ExternalPackageProperties {
                name: project.name.map(Spanned::take),
                version: project
                    .version
                    .and_then(|v| v.take().to_string().parse().ok())
                    .or(poetry.version.and_then(|v| v.parse().ok())),
                description: project
                    .description
                    .map(Spanned::take)
                    .or(poetry.description),
                authors: project.authors.map(contacts_to_authors).or(poetry.authors),
                license: None,
                license_file: None,
                readme: None,
                homepage: None,
                repository: None,
                documentation: None,
            },
            workspace,
            root_directory,
        )
    }

    #[allow(clippy::result_large_err)]
    pub fn into_workspace_manifest(
        self,
        root_directory: Option<&Path>,
    ) -> Result<(WorkspaceManifest, Option<PackageManifest>, Vec<Warning>), TomlError> {
        let PyProjectToml {
            project,
            dependency_groups,
            ..
        } = self.project;

        // Load the data nested under '[tool.pixi]' as pixi manifest
        let Some(Tool {
            pixi: Some(pixi),
            poetry,
        }) = self.tool
        else {
            return Err(TomlError::MissingField("tool.pixi".into(), None));
        };

        // Extract the values we are interested in from the pyproject.toml
        let project = project.map(PyProjectFields::from).unwrap_or_default();

        // Extract some of the values we are interested in from the poetry table.
        let poetry = poetry.unwrap_or_default();

        // Define an iterator over both optional dependencies and dependency groups
        let project_name = project.name.map(Spanned::take);
        let pypi_dependency_groups = Self::extract_dependency_groups(
            dependency_groups,
            project.optional_dependencies,
            project_name.as_deref(),
        )?;

        // Convert the TOML document into a pixi manifest.
        // TODO:  would be nice to add license, license-file, readme, homepage,
        // repository, documentation, regarding the above, the types are a bit
        // different than we expect, so the conversion is not straightforward we
        // could change these types or we can convert. Let's decide when we make it.
        // etc.
        let implicit_pypi_features = pypi_dependency_groups
            .iter()
            .map(|(name, _)| {
                (
                    FeatureName::from(name.clone()),
                    Feature::new(FeatureName::from(name.clone())),
                )
            })
            .collect();
        let (mut workspace_manifest, package_manifest, warnings) = pixi.into_workspace_manifest(
            ExternalWorkspaceProperties {
                name: project_name,
                version: project
                    .version
                    .and_then(|v| v.take().to_string().parse().ok())
                    .or(poetry.version.and_then(|v| v.parse().ok())),
                description: project
                    .description
                    .map(Spanned::take)
                    .or(poetry.description),
                authors: project.authors.map(contacts_to_authors).or(poetry.authors),
                license: None,
                license_file: None,
                readme: None,
                homepage: None,
                repository: None,
                documentation: None,
                features: implicit_pypi_features,
            },
            root_directory,
        )?;

        // Add python as dependency based on the `project.requires_python` property
        let python_spec = project.requires_python;

        let target = workspace_manifest
            .default_feature_mut()
            .targets
            .default_mut();
        let python = PackageName::from_str("python").unwrap();
        // If the target doesn't have any python dependency, we add it from the
        // `requires-python`
        if !target.has_dependency(&python, SpecType::Run, None) {
            target.add_dependency(
                &python,
                &version_or_url_to_spec(&python_spec.map(Spanned::take)).unwrap(),
                SpecType::Run,
            );
        } else if let Some(_spec) = python_spec {
            if target.has_dependency(&python, SpecType::Run, None) {
                // TODO: implement some comparison or spec merging logic here
                tracing::info!(
                    "Overriding the requires-python with the one defined in pixi dependencies"
                )
            }
        }

        // Add pyproject dependencies as pypi dependencies
        if let Some(deps) = project.dependencies {
            for requirement in deps.iter() {
                target
                    .try_add_pep508_dependency(
                        &requirement.value,
                        None,
                        DependencyOverwriteBehavior::Error,
                    )
                    .map_err(|err| {
                        GenericError::new(format!("{}", err)).with_span(requirement.span.into())
                    })?;
            }
        }

        // For each group of optional dependency or dependency group, add pypi
        // dependencies
        for (group, reqs) in pypi_dependency_groups {
            let feature_name = FeatureName::from(group.to_string());
            let target = workspace_manifest
                .features
                .entry(feature_name.clone())
                .or_insert_with(move || Feature::new(feature_name))
                .targets
                .default_mut();
            for requirement in reqs.iter() {
                target
                    .try_add_pep508_dependency(
                        requirement,
                        None,
                        DependencyOverwriteBehavior::Error,
                    )
                    .map_err(|err| GenericError::new(format!("{}", err)))?;
            }
        }

        Ok((workspace_manifest, package_manifest, warnings))
    }

    fn extract_dependency_groups(
        dependency_groups: Option<Spanned<TomlDependencyGroups>>,
        optional_dependencies: Option<Spanned<TomlOptionalDependencies>>,
        project_name: Option<&str>,
    ) -> Result<Vec<(String, Vec<Requirement>)>, TomlError> {
        let mut result = Vec::new();

        if let Some(Spanned { span, value }) = optional_dependencies {
            let resolved = value
                .0
                .resolve(project_name)
                .map_err(|err| GenericError::new(err.to_string()).with_span(span.into()))?;
            result.extend(resolved);
        }

        if let Some(Spanned { span, value }) = dependency_groups {
            let resolved = value
                .0
                .resolve()
                .map_err(|err| GenericError::new(err.to_string()).with_span(span.into()))?;
            result.extend(resolved);
        }

        Ok(result)
    }
}

/// Try to return a NamelessMatchSpec from a pep508_rs::VersionOrUrl
/// This will only work if it is not URL and the VersionSpecifier can
/// successfully be interpreted as a NamelessMatchSpec.version
fn version_or_url_to_spec(
    version: &Option<VersionSpecifiers>,
) -> Result<PixiSpec, RequirementConversionError> {
    match version {
        // TODO: avoid going through string representation for conversion
        Some(v) => {
            let version_string = v.to_string();
            // Double equals works a bit different in conda vs. python
            let version_string = version_string.strip_prefix("==").unwrap_or(&version_string);
            Ok(VersionSpec::from_str(version_string, Lenient)?.into())
        }
        None => Ok(PixiSpec::default()),
    }
}

/// Converts [`Contact`] from pyproject.toml to a representation that is used in
/// pixi.
fn contacts_to_authors(contacts: Vec<Spanned<TomlContact>>) -> Vec<String> {
    contacts
        .into_iter()
        .map(|contact| match contact.take().into_inner() {
            Contact::NameEmail { name, email } => format!("{} <{}>", name, email),
            Contact::Name { name } => name.clone(),
            Contact::Email { email } => email.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use pep440_rs::VersionSpecifiers;
    use rattler_conda_types::{ParseStrictness, VersionSpec};

    use crate::{ManifestSource, Manifests};

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

    #[test]
    fn test_build_manifest() {
        let source = ManifestSource::PyProjectToml(PYPROJECT_FULL.to_string());
        let _ = Manifests::from_workspace_source(source.with_provenance_from_kind()).unwrap();
    }

    #[test]
    fn test_version_url_to_matchspec() {
        fn cmp(v1: &str, v2: &str) {
            let v = VersionSpecifiers::from_str(v1).unwrap();
            let matchspec = super::version_or_url_to_spec(&Some(v)).unwrap();
            let version_spec = matchspec.as_version_spec().unwrap();
            let vspec = VersionSpec::from_str(v2, ParseStrictness::Strict).unwrap();
            assert_eq!(version_spec, &vspec);
        }

        cmp("==3.12", "==3.12");
        cmp("==3.12.*", "3.12.*");
        // rest should work just fine
        cmp(">=3.12", ">=3.12");
        cmp(">=3.10,<3.12", ">=3.10,<3.12");
        cmp("~=3.12", "~=3.12");
    }
}
