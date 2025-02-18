use std::{fmt, str::FromStr, sync::Arc};

use miette::{Diagnostic, NamedSource};
use pixi_consts::consts;
use pixi_spec::PixiSpec;
use rattler_conda_types::{PackageName, Platform};
use thiserror::Error;
use toml_edit::{value, Array, DocumentMut, Item, Table, Value};

use crate::{
    manifests::table_name::TableName, pypi::PyPiPackageName, toml::TomlDocument,
    utils::WithSourceCode, FeatureName, LibCSystemRequirement, ManifestKind, ManifestProvenance,
    PyPiRequirement, PypiDependencyLocation, SpecType, SystemRequirements, Task, TomlError,
};

/// Discriminates between a 'pixi.toml' and a 'pyproject.toml' manifest.
#[derive(Debug, Clone)]
pub enum ManifestDocument {
    PyProjectToml(TomlDocument),
    PixiToml(TomlDocument),
}

impl fmt::Display for ManifestDocument {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ManifestDocument::PyProjectToml(document) => write!(f, "{}", document),
            ManifestDocument::PixiToml(document) => write!(f, "{}", document),
        }
    }
}

/// An error that is returned when trying to parse a manifest file.
#[derive(Debug, Error, Diagnostic)]
pub enum ManifestDocumentError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(#[from] Box<WithSourceCode<TomlError, NamedSource<Arc<str>>>>),
}

impl ManifestDocument {
    /// Returns a new empty pixi manifest.
    #[cfg(test)]
    pub fn empty_pixi() -> Self {
        use std::str::FromStr;

        use toml_edit::DocumentMut;

        let empty_content = r#"
        [project]
        name = "test"
        channels = []
        platforms = []
        "#
        .lines()
        .map(|line| line.trim_start())
        .collect::<Vec<_>>()
        .join("\n");

        ManifestDocument::PixiToml(TomlDocument::new(
            DocumentMut::from_str(empty_content.as_str()).unwrap(),
        ))
    }

    /// Returns a new empty pyproject manifest.
    #[cfg(test)]
    pub fn empty_pyproject() -> Self {
        use std::str::FromStr;

        use toml_edit::DocumentMut;

        let empty_content = r#"
        [project]
        name = "test"
        [tool.pixi.project]
        channels = []
        platforms = []
        "#
        .lines()
        .map(|line| line.trim_start())
        .collect::<Vec<_>>()
        .join("\n");

        ManifestDocument::PyProjectToml(TomlDocument::new(
            DocumentMut::from_str(empty_content.as_str()).unwrap(),
        ))
    }

    /// Converts the document into a string with provenance.
    #[cfg(test)]
    pub(crate) fn into_source_with_provenance(self) -> crate::WithProvenance<String> {
        use std::path::PathBuf;

        use crate::{AssociateProvenance, ManifestProvenance};

        let kind = self.kind();
        let document = match self {
            ManifestDocument::PyProjectToml(document) => document,
            ManifestDocument::PixiToml(document) => document,
        };
        document
            .to_string()
            .with_provenance(ManifestProvenance::new(
                PathBuf::from(consts::PYPROJECT_MANIFEST),
                kind,
            ))
    }

    /// Reads the contents of the manifest from a provenance.
    pub fn from_provenance(provenance: &ManifestProvenance) -> Result<Self, ManifestDocumentError> {
        // Read the contents of the file
        let contents = provenance.read()?.into_inner();

        // Parse the contents
        let toml = match DocumentMut::from_str(&contents) {
            Ok(document) => TomlDocument::new(document),
            Err(err) => {
                return Err(Box::new(WithSourceCode {
                    source: NamedSource::new(
                        provenance.absolute_path().to_string_lossy(),
                        Arc::from(contents),
                    ),
                    error: TomlError::from(err),
                })
                .into())
            }
        };

        match provenance.kind {
            ManifestKind::Pyproject => Ok(ManifestDocument::PyProjectToml(toml)),
            ManifestKind::Pixi => Ok(ManifestDocument::PixiToml(toml)),
        }
    }

    /// Returns the type of the manifest.
    pub fn kind(&self) -> ManifestKind {
        match self {
            ManifestDocument::PyProjectToml(_) => ManifestKind::Pyproject,
            ManifestDocument::PixiToml(_) => ManifestKind::Pixi,
        }
    }

    /// Returns the file name of the manifest
    #[cfg(test)]
    pub fn file_name(&self) -> &'static str {
        self.kind().file_name()
    }

    fn table_prefix(&self) -> Option<&'static str> {
        match self {
            ManifestDocument::PyProjectToml(_) => Some(consts::PYPROJECT_PIXI_PREFIX),
            ManifestDocument::PixiToml(_) => None,
        }
    }

    fn manifest_mut(&mut self) -> &mut TomlDocument {
        match self {
            ManifestDocument::PyProjectToml(document) => document,
            ManifestDocument::PixiToml(document) => document,
        }
    }

    /// Returns the inner TOML document
    pub fn manifest(&self) -> &TomlDocument {
        match self {
            ManifestDocument::PyProjectToml(document) => document,
            ManifestDocument::PixiToml(document) => document,
        }
    }

    /// Returns `true` if the manifest is a 'pyproject.toml' manifest.
    pub fn is_pyproject_toml(&self) -> bool {
        matches!(self, ManifestDocument::PyProjectToml(_))
    }

    /// Detect the table name to use when querying elements of the manifest.
    fn detect_table_name(&self) -> &'static str {
        if self.manifest().as_table().contains_key("workspace") {
            "workspace"
        } else {
            "project"
        }
    }

    /// Returns a mutable reference to the specified array either in project or
    /// feature.
    pub fn get_array_mut(
        &mut self,
        array_name: &str,
        feature_name: &FeatureName,
    ) -> Result<&mut Array, TomlError> {
        // TODO: When `[package]` will become a standalone table, this method
        // should be refactored to determine the priority of the table to use
        // The spec is described here:
        // https://github.com/prefix-dev/pixi/issues/2807#issuecomment-2577826553
        let table = match feature_name {
            FeatureName::Default => Some(self.detect_table_name()),
            FeatureName::Named(_) => None,
        };

        let table_name = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(feature_name))
            .with_table(table);

        self.manifest_mut()
            .get_or_insert_toml_array_mut(table_name.to_string().as_str(), array_name)
    }

    fn as_table_mut(&mut self) -> &mut Table {
        match self {
            ManifestDocument::PyProjectToml(document) => document.as_table_mut(),
            ManifestDocument::PixiToml(document) => document.as_table_mut(),
        }
    }

    /// Removes a pypi dependency from the TOML manifest from native pyproject
    /// arrays and/or pixi tables as required.
    ///
    /// If will be a no-op if the dependency is not found.
    pub fn remove_pypi_dependency(
        &mut self,
        dep: &PyPiPackageName,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), TomlError> {
        // For 'pyproject.toml' manifest, try and remove the dependency from native
        // arrays
        let remove_requirement =
            |source: &mut ManifestDocument, table, array_name| -> Result<(), TomlError> {
                let array = source
                    .manifest_mut()
                    .get_mut_toml_array(table, array_name)?;
                if let Some(array) = array {
                    array.retain(|x| {
                        let req: pep508_rs::Requirement = x
                            .as_str()
                            .unwrap_or("")
                            .parse()
                            .expect("should be a valid pep508 dependency");
                        let name = PyPiPackageName::from_normalized(req.name);
                        name != *dep
                    });
                    if array.is_empty() {
                        source
                            .manifest_mut()
                            .get_or_insert_nested_table(table)?
                            .remove(array_name);
                    }
                }
                Ok(())
            };

        match self {
            ManifestDocument::PyProjectToml(_) if feature_name.is_default() => {
                remove_requirement(self, "project", "dependencies")?;
            }
            ManifestDocument::PyProjectToml(_) => {
                let name = feature_name.to_string();
                remove_requirement(self, "project.optional-dependencies", &name)?;
                remove_requirement(self, "dependency-groups", &name)?;
            }
            _ => (),
        };

        // For both 'pyproject.toml' and 'pixi.toml' manifest,
        // try and remove the dependency from pixi native tables
        let table_name = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(feature_name))
            .with_platform(platform.as_ref())
            .with_table(Some(consts::PYPI_DEPENDENCIES));

        self.manifest_mut()
            .get_or_insert_nested_table(table_name.to_string().as_str())
            .map(|t| t.remove(dep.as_source()))?;
        Ok(())
    }

    /// Removes a conda or pypi dependency from the TOML manifest's pixi table
    /// for either a 'pyproject.toml' and 'pixi.toml'
    ///
    /// If will be a no-op if the dependency is not found
    pub fn remove_dependency(
        &mut self,
        dep: &PackageName,
        spec_type: SpecType,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), TomlError> {
        let table_name = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(feature_name))
            .with_platform(platform.as_ref())
            .with_table(Some(spec_type.name()));

        self.manifest_mut()
            .get_or_insert_nested_table(table_name.to_string().as_str())
            .map(|t| t.remove(dep.as_source()))?;
        Ok(())
    }

    /// Adds a conda dependency to the TOML manifest
    ///
    /// If a dependency with the same name already exists, it will be replaced.
    pub fn add_dependency(
        &mut self,
        name: &PackageName,
        spec: &PixiSpec,
        spec_type: SpecType,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), TomlError> {
        let dependency_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_platform(platform.as_ref())
            .with_feature_name(Some(feature_name))
            .with_table(Some(spec_type.name()));

        self.manifest_mut()
            .get_or_insert_nested_table(dependency_table.to_string().as_str())
            .map(|t| t.insert(name.as_normalized(), Item::Value(spec.to_toml_value())))?;

        Ok(())
    }

    /// Adds a pypi dependency to the TOML manifest
    ///
    /// If a pypi dependency with the same name already exists, it will be
    /// replaced.
    pub fn add_pypi_dependency(
        &mut self,
        requirement: &pep508_rs::Requirement,
        platform: Option<Platform>,
        feature_name: &FeatureName,
        editable: Option<bool>,
        location: &Option<PypiDependencyLocation>,
    ) -> Result<(), TomlError> {
        // Pypi dependencies can be stored in different places in pyproject.toml
        // manifests so we remove any potential dependency of the same name
        // before adding it back
        if matches!(self, ManifestDocument::PyProjectToml(_)) {
            self.remove_pypi_dependency(
                &PyPiPackageName::from_normalized(requirement.name.clone()),
                platform,
                feature_name,
            )?;
        }

        // The '[pypi-dependencies]' or '[tool.pixi.pypi-dependencies]' table is
        // selected
        //  - For 'pixi.toml' manifests where it is the only choice
        //  - When explicitly requested
        //  - When a specific platform is requested, as markers are not supported (https://github.com/prefix-dev/pixi/issues/2149)
        //  - When an editable install is requested
        if matches!(self, ManifestDocument::PixiToml(_))
            || matches!(location, Some(PypiDependencyLocation::PixiPypiDependencies))
            || platform.is_some()
            || editable.is_some_and(|e| e)
        {
            let mut pypi_requirement =
                PyPiRequirement::try_from(requirement.clone()).map_err(Box::new)?;
            if let Some(editable) = editable {
                pypi_requirement.set_editable(editable);
            }

            let dependency_table = TableName::new()
                .with_prefix(self.table_prefix())
                .with_platform(platform.as_ref())
                .with_feature_name(Some(feature_name))
                .with_table(Some(consts::PYPI_DEPENDENCIES));

            self.manifest_mut()
                .get_or_insert_nested_table(dependency_table.to_string().as_str())?
                .insert(
                    requirement.name.as_ref(),
                    Item::Value(pypi_requirement.into()),
                );
            return Ok(());
        }

        // Otherwise:
        //   - the [project.dependencies] array is selected for the default feature
        //   - the [dependency-groups.feature_name] array is selected unless
        //   - optional-dependencies is explicitly requested as location
        let add_requirement =
            |source: &mut ManifestDocument, table, array| -> Result<(), TomlError> {
                source
                    .manifest_mut()
                    .get_or_insert_toml_array_mut(table, array)?
                    .push(requirement.to_string());
                Ok(())
            };
        if feature_name.is_default()
            || matches!(location, Some(PypiDependencyLocation::Dependencies))
        {
            add_requirement(self, "project", "dependencies")?
        } else if matches!(location, Some(PypiDependencyLocation::OptionalDependencies)) {
            add_requirement(
                self,
                "project.optional-dependencies",
                &feature_name.to_string(),
            )?
        } else {
            add_requirement(self, "dependency-groups", &feature_name.to_string())?
        }
        Ok(())
    }

    /// Determines the location of a PyPi dependency within the manifest.
    ///
    /// This method checks various sections of the manifest to locate the
    /// specified PyPi dependency. It searches in the following order:
    /// 1. `pypi-dependencies` table in the manifest.
    /// 2. `project.dependencies` array in the manifest.
    /// 3. `project.optional-dependencies` array in the manifest.
    /// 4. `dependency-groups` array in the manifest.
    ///
    /// # Arguments
    ///
    /// * `dep` - The name of the PyPi package to locate.
    /// * `platform` - An optional platform specification.
    /// * `feature_name` - The name of the feature to which the dependency
    ///   belongs.
    ///
    /// # Returns
    ///
    /// An `Option` containing the `PypiDependencyLocation` if the dependency is
    /// found, or `None` if it is not found in any of the checked sections.
    pub fn pypi_dependency_location(
        &self,
        package_name: &PyPiPackageName,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Option<PypiDependencyLocation> {
        // For both 'pyproject.toml' and 'pixi.toml' manifest,
        // try and to get `pypi-dependency`
        let table_name = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(feature_name))
            .with_platform(platform.as_ref())
            .with_table(Some(consts::PYPI_DEPENDENCIES));

        let pypi_dependency_table = self
            .manifest()
            .get_nested_table(table_name.to_string().as_str())
            .ok();

        if pypi_dependency_table
            .and_then(|table| table.get(package_name.as_source()))
            .is_some()
        {
            return Some(PypiDependencyLocation::PixiPypiDependencies);
        }

        if self
            .manifest()
            .get_toml_array("project", "dependencies")
            .is_ok()
        {
            return Some(PypiDependencyLocation::Dependencies);
        }
        let name = feature_name.to_string();

        if self
            .manifest()
            .get_toml_array("project.optional-dependencies", &name)
            .is_ok()
        {
            return Some(PypiDependencyLocation::OptionalDependencies);
        }

        if self
            .manifest()
            .get_toml_array("dependency-groups", &name)
            .is_ok()
        {
            return Some(PypiDependencyLocation::DependencyGroups);
        }

        None
    }

    /// Removes a task from the TOML manifest
    pub fn remove_task(
        &mut self,
        name: &str,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), TomlError> {
        // Get the task table either from the target platform or the default tasks.
        // If it does not exist in TOML, consider this ok as we want to remove it
        // anyways
        let task_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_platform(platform.as_ref())
            .with_feature_name(Some(feature_name))
            .with_table(Some("tasks"));

        self.manifest_mut()
            .get_or_insert_nested_table(task_table.to_string().as_str())?
            .remove(name);

        Ok(())
    }

    /// Adds a task to the TOML manifest
    pub fn add_task(
        &mut self,
        name: &str,
        task: Task,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), TomlError> {
        // Get the task table either from the target platform or the default tasks.
        let task_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_platform(platform.as_ref())
            .with_feature_name(Some(feature_name))
            .with_table(Some("tasks"));

        self.manifest_mut()
            .get_or_insert_nested_table(task_table.to_string().as_str())?
            .insert(name, task.into());

        Ok(())
    }

    /// Adds an environment to the manifest
    pub fn add_environment(
        &mut self,
        name: impl Into<String>,
        features: Option<Vec<String>>,
        solve_group: Option<String>,
        no_default_features: bool,
    ) -> Result<(), TomlError> {
        // Construct the TOML item
        let item = if solve_group.is_some() || no_default_features {
            let mut table = toml_edit::InlineTable::new();
            if let Some(features) = features {
                table.insert("features", Array::from_iter(features).into());
            }
            if let Some(solve_group) = solve_group {
                table.insert("solve-group", solve_group.into());
            }
            if no_default_features {
                table.insert("no-default-feature", true.into());
            }
            Item::Value(table.into())
        } else {
            Item::Value(Value::Array(Array::from_iter(
                features.into_iter().flatten(),
            )))
        };

        let env_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(&FeatureName::Default))
            .with_table(Some("environments"));

        // Insert into the environment table
        self.manifest_mut()
            .get_or_insert_nested_table(env_table.to_string().as_str())?
            .insert(&name.into(), item);

        Ok(())
    }

    /// Removes an environment from the manifest. Returns `true` if the
    /// environment was removed.
    pub fn remove_environment(&mut self, name: &str) -> Result<bool, TomlError> {
        let env_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(&FeatureName::Default))
            .with_table(Some("environments"));

        Ok(self
            .manifest_mut()
            .get_or_insert_nested_table(env_table.to_string().as_str())?
            .remove(name)
            .is_some())
    }

    pub fn add_system_requirements(
        &mut self,
        system_requirements: &SystemRequirements,
        feature_name: &FeatureName,
    ) -> Result<bool, TomlError> {
        let system_requirements_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(feature_name))
            .with_table(Some(consts::SYSTEM_REQUIREMENTS));

        let mut inserted = false;

        if let Some(linux) = &system_requirements.linux {
            inserted |= self
                .manifest_mut()
                .get_or_insert_nested_table(system_requirements_table.to_string().as_str())?
                .insert("linux", toml_edit::Item::from(linux.to_string()))
                .is_some();
        }

        if let Some(cuda) = &system_requirements.cuda {
            inserted |= self
                .manifest_mut()
                .get_or_insert_nested_table(system_requirements_table.to_string().as_str())?
                .insert("cuda", toml_edit::Item::from(cuda.to_string()))
                .is_some();
        }

        if let Some(macos) = &system_requirements.macos {
            inserted |= self
                .manifest_mut()
                .get_or_insert_nested_table(system_requirements_table.to_string().as_str())?
                .insert("macos", toml_edit::Item::from(macos.to_string()))
                .is_some();
        }

        if let Some(libc) = &system_requirements.libc {
            match libc {
                LibCSystemRequirement::GlibC(version) => {
                    inserted |= self
                        .manifest_mut()
                        .get_or_insert_nested_table(system_requirements_table.to_string().as_str())?
                        .insert("libc", toml_edit::Item::from(version.to_string()))
                        .is_some();
                }
                LibCSystemRequirement::OtherFamily(family_and_version) => {
                    if let Some(family) = &family_and_version.family {
                        let mut libc_table = Table::new();
                        libc_table.insert("family", toml_edit::value(family));
                        libc_table.insert(
                            "version",
                            toml_edit::Item::from(family_and_version.version.clone().to_string()),
                        );
                        inserted |= self
                            .manifest_mut()
                            .get_or_insert_nested_table(
                                system_requirements_table.to_string().as_str(),
                            )?
                            .insert(
                                "libc",
                                toml_edit::Item::from(libc_table.into_inline_table()),
                            )
                            .is_some();
                    } else {
                        inserted |= self
                            .manifest_mut()
                            .get_or_insert_nested_table(
                                system_requirements_table.to_string().as_str(),
                            )?
                            .insert(
                                "libc",
                                toml_edit::Item::from(
                                    family_and_version.version.clone().to_string(),
                                ),
                            )
                            .is_some();
                    }
                }
            }
        }

        if let Some(archspec) = &system_requirements.archspec {
            inserted |= self
                .manifest_mut()
                .get_or_insert_nested_table(system_requirements_table.to_string().as_str())?
                .insert("archspec", archspec.into())
                .is_some();
        }

        Ok(inserted)
    }

    /// Sets the name of the project
    pub fn set_name(&mut self, name: &str) {
        self.as_table_mut()["project"]["name"] = value(name);
    }

    /// Sets the description of the project
    pub fn set_description(&mut self, description: &str) {
        self.as_table_mut()["project"]["description"] = value(description);
    }

    /// Sets the version of the project
    pub fn set_version(&mut self, version: &str) {
        self.as_table_mut()["project"]["version"] = value(version);
    }
}
