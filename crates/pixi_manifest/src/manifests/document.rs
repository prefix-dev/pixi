use std::{fmt, str::FromStr, sync::Arc};

use miette::{Diagnostic, NamedSource};
use pixi_consts::consts;
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use rattler_conda_types::{PackageName, Platform};
use thiserror::Error;
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use crate::{
    FeatureName, LibCSystemRequirement, ManifestKind, ManifestProvenance, PypiDependencyLocation,
    SpecType, SystemRequirements, Task, TomlError, manifests::table_name::TableName,
    toml::TomlDocument, utils::WithSourceCode,
};

/// Discriminates between a 'pixi.toml' and a 'pyproject.toml' manifest.
#[derive(Debug, Clone)]
pub enum ManifestDocument {
    PyProjectToml(TomlDocument),
    PixiToml(TomlDocument),
    MojoProjectToml(TomlDocument),
}

impl fmt::Display for ManifestDocument {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ManifestDocument::PyProjectToml(document) => write!(f, "{}", document),
            ManifestDocument::PixiToml(document) => write!(f, "{}", document),
            ManifestDocument::MojoProjectToml(document) => write!(f, "{}", document),
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
            ManifestDocument::MojoProjectToml(document) => document,
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
                .into());
            }
        };

        match provenance.kind {
            ManifestKind::Pyproject => Ok(ManifestDocument::PyProjectToml(toml)),
            ManifestKind::Pixi => Ok(ManifestDocument::PixiToml(toml)),
            ManifestKind::MojoProject => Ok(ManifestDocument::MojoProjectToml(toml)),
        }
    }

    /// Returns the type of the manifest.
    pub fn kind(&self) -> ManifestKind {
        match self {
            ManifestDocument::PyProjectToml(_) => ManifestKind::Pyproject,
            ManifestDocument::PixiToml(_) => ManifestKind::Pixi,
            ManifestDocument::MojoProjectToml(_) => ManifestKind::MojoProject,
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
            ManifestDocument::MojoProjectToml(_) => None,
        }
    }

    fn manifest_mut(&mut self) -> &mut TomlDocument {
        match self {
            ManifestDocument::PyProjectToml(document) => document,
            ManifestDocument::PixiToml(document) => document,
            ManifestDocument::MojoProjectToml(document) => document,
        }
    }

    /// Returns the inner TOML document
    pub fn manifest(&self) -> &TomlDocument {
        match self {
            ManifestDocument::PyProjectToml(document) => document,
            ManifestDocument::PixiToml(document) => document,
            ManifestDocument::MojoProjectToml(document) => document,
        }
    }

    /// Returns `true` if the manifest is a 'pyproject.toml' manifest.
    pub fn is_pyproject_toml(&self) -> bool {
        matches!(self, ManifestDocument::PyProjectToml(_))
    }

    /// Detect the table name to use when querying elements of the manifest.
    fn detect_table_name(&self) -> &'static str {
        if self.manifest().as_table().contains_key("workspace") {
            // pixi.toml
            "workspace"
        } else if self
            .manifest()
            .as_table()
            .get("tool")
            .and_then(|t| t.get("pixi"))
            .and_then(|t| t.get("workspace"))
            .is_some()
        {
            // pyproject.toml
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
        let table = feature_name.is_default().then(|| self.detect_table_name());

        let table_name = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(feature_name))
            .with_table(table);

        self.manifest_mut()
            .get_or_insert_toml_array_mut(&table_name.as_keys(), array_name)
    }

    fn as_table_mut(&mut self) -> &mut Table {
        match self {
            ManifestDocument::PyProjectToml(document) => document.as_table_mut(),
            ManifestDocument::PixiToml(document) => document.as_table_mut(),
            ManifestDocument::MojoProjectToml(document) => document.as_table_mut(),
        }
    }

    /// Removes a pypi dependency from the TOML manifest from native pyproject
    /// arrays and/or pixi tables as required.
    ///
    /// If will be a no-op if the dependency is not found.
    pub fn remove_pypi_dependency(
        &mut self,
        dep: &PypiPackageName,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), TomlError> {
        // For 'pyproject.toml' manifest, try and remove the dependency from native
        // arrays
        match self {
            ManifestDocument::PyProjectToml(_) if feature_name.is_default() => {
                self.remove_pypi_requirement(&["project"], "dependencies", dep)?;
            }
            ManifestDocument::PyProjectToml(_) => {
                let name = feature_name.to_string();
                self.remove_pypi_requirement(&["project", "optional-dependencies"], &name, dep)?;
                self.remove_pypi_requirement(&["dependency-groups"], &name, dep)?;
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
            .get_or_insert_nested_table(&table_name.as_keys())
            .map(|t| t.remove(dep.as_source()))?;
        Ok(())
    }

    /// Removes a pypi requirement from a particular array.
    fn remove_pypi_requirement(
        &mut self,
        table_parts: &[&str],
        array_name: &str,
        dependency_name: &PypiPackageName,
    ) -> Result<(), TomlError> {
        let array = self
            .manifest_mut()
            .get_mut_toml_array(table_parts, array_name)?;
        if let Some(array) = array {
            array.retain(|x| {
                let req: pep508_rs::Requirement = x
                    .as_str()
                    .unwrap_or("")
                    .parse()
                    .expect("should be a valid pep508 dependency");
                let name = PypiPackageName::from_normalized(req.name);
                name != *dependency_name
            });
            if array.is_empty() {
                self.manifest_mut()
                    .get_or_insert_nested_table(table_parts)?
                    .remove(array_name);
            }
        }
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
            .get_or_insert_nested_table(&table_name.as_keys())
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
            .get_or_insert_nested_table(&dependency_table.as_keys())
            .map(|t| {
                let mut new_value = spec.to_toml_value();

                // Check if there is an existing entry that is represented by an inline value.
                let existing_value = t.iter_mut().find_map(|(key, value)| {
                    let package_key_name = PackageName::from_str(key.get()).ok()?;
                    if package_key_name == *name {
                        value.as_value_mut()
                    } else {
                        None
                    }
                });

                // If there exists an existing value, we update it with the new value, but we
                // keep the decoration.
                if let Some(existing_value) = existing_value {
                    *new_value.decor_mut() = existing_value.decor().clone();
                    *existing_value = new_value;
                } else {
                    // Otherwise, just reinsert the value. This might overwrite an existing
                    // decorations.
                    t.insert(name.as_normalized(), Item::Value(new_value));
                }
            })?;

        Ok(())
    }

    /// Adds a pypi dependency to the TOML manifest
    ///
    /// If a pypi dependency with the same name already exists, it will be
    /// replaced.
    pub fn add_pypi_dependency(
        &mut self,
        requirement: &pep508_rs::Requirement,
        pixi_requirement: Option<&PixiPypiSpec>,
        platform: Option<Platform>,
        feature_name: &FeatureName,
        editable: Option<bool>,
        location: Option<PypiDependencyLocation>,
    ) -> Result<(), TomlError> {
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
                PixiPypiSpec::try_from((requirement.clone(), pixi_requirement.cloned()))
                    .map_err(Box::new)?;
            if let Some(editable) = editable {
                pypi_requirement.set_editable(editable);
            }

            let dependency_table_name = TableName::new()
                .with_prefix(self.table_prefix())
                .with_platform(platform.as_ref())
                .with_feature_name(Some(feature_name))
                .with_table(Some(consts::PYPI_DEPENDENCIES));

            let table = self
                .manifest_mut()
                .get_or_insert_nested_table(&dependency_table_name.as_keys())?;

            let mut new_value = Value::from(pypi_requirement);

            // Check if there exists an existing entry in the table that we should overwrite
            // instead.
            let existing_value = table.iter_mut().find_map(|(key, value)| {
                let existing_name = pep508_rs::PackageName::from_str(key.get()).ok()?;
                if existing_name == requirement.name {
                    value.as_value_mut()
                } else {
                    None
                }
            });

            // If there exists an existing entry, we overwrite it but keep the decoration.
            if let Some(existing_value) = existing_value {
                *new_value.decor_mut() = existing_value.decor().clone();
                *existing_value = new_value;
            } else {
                table.insert(requirement.name.as_ref(), Item::Value(new_value));
            }

            // Remove the entry from the project native array.
            self.remove_pypi_requirement(
                &["project"],
                "dependencies",
                &PypiPackageName::from_normalized(requirement.name.clone()),
            )?;

            return Ok(());
        }

        // Otherwise:
        //   - the [project.dependencies] array is selected for the default feature
        //   - the [dependency-groups.feature_name] array is selected unless
        //   - optional-dependencies is explicitly requested as location
        let add_requirement = |source: &mut ManifestDocument,
                               table_parts: &[&str],
                               array: &str|
         -> Result<(), TomlError> {
            let array = source
                .manifest_mut()
                .get_or_insert_toml_array_mut(table_parts, array)?;

            // Check if there is an existing entry that we should replace. Replacing will
            // preserve any existing formatting.
            let existing_entry_idx = array.iter().position(|item| {
                let Ok(req): Result<pep508_rs::Requirement, _> =
                    item.as_str().unwrap_or_default().parse()
                else {
                    return false;
                };
                req.name == requirement.name
            });

            if let Some(idx) = existing_entry_idx {
                array.replace(idx, requirement.to_string());
            } else {
                array.push(requirement.to_string());
            }
            Ok(())
        };
        if feature_name.is_default()
            || matches!(location, Some(PypiDependencyLocation::Dependencies))
        {
            add_requirement(self, &["project"], "dependencies")?
        } else if matches!(location, Some(PypiDependencyLocation::OptionalDependencies)) {
            add_requirement(
                self,
                &["project", "optional-dependencies"],
                &feature_name.to_string(),
            )?
        } else {
            add_requirement(self, &["dependency-groups"], &feature_name.to_string())?
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
        package_name: &PypiPackageName,
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

        let pypi_dependency_table = self.manifest().get_nested_table(&table_name.as_keys()).ok();

        if pypi_dependency_table
            .and_then(|table| table.get(package_name.as_source()))
            .is_some()
        {
            return Some(PypiDependencyLocation::PixiPypiDependencies);
        }

        if self
            .manifest()
            .get_toml_array(&["project"], "dependencies")
            .is_ok()
        {
            return Some(PypiDependencyLocation::Dependencies);
        }
        let name = feature_name.to_string();

        if self
            .manifest()
            .get_toml_array(&["project", "optional-dependencies"], &name)
            .is_ok()
        {
            return Some(PypiDependencyLocation::OptionalDependencies);
        }

        if self
            .manifest()
            .get_toml_array(&["dependency-groups"], &name)
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
            .get_or_insert_nested_table(&task_table.as_keys())?
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
            .get_or_insert_nested_table(&task_table.as_keys())?
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
            .with_feature_name(Some(&FeatureName::DEFAULT))
            .with_table(Some("environments"));

        // Insert into the environment table
        self.manifest_mut()
            .get_or_insert_nested_table(&env_table.as_keys())?
            .insert(&name.into(), item);

        Ok(())
    }

    /// Removes an environment from the manifest. Returns `true` if the
    /// environment was removed.
    pub fn remove_environment(&mut self, name: &str) -> Result<bool, TomlError> {
        let env_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(&FeatureName::DEFAULT))
            .with_table(Some("environments"));

        Ok(self
            .manifest_mut()
            .get_or_insert_nested_table(&env_table.as_keys())?
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
                .get_or_insert_nested_table(&system_requirements_table.as_keys())?
                .insert("linux", toml_edit::Item::from(linux.to_string()))
                .is_some();
        }

        if let Some(cuda) = &system_requirements.cuda {
            inserted |= self
                .manifest_mut()
                .get_or_insert_nested_table(&system_requirements_table.as_keys())?
                .insert("cuda", toml_edit::Item::from(cuda.to_string()))
                .is_some();
        }

        if let Some(macos) = &system_requirements.macos {
            inserted |= self
                .manifest_mut()
                .get_or_insert_nested_table(&system_requirements_table.as_keys())?
                .insert("macos", toml_edit::Item::from(macos.to_string()))
                .is_some();
        }

        if let Some(libc) = &system_requirements.libc {
            match libc {
                LibCSystemRequirement::GlibC(version) => {
                    inserted |= self
                        .manifest_mut()
                        .get_or_insert_nested_table(&system_requirements_table.as_keys())?
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
                            .get_or_insert_nested_table(&system_requirements_table.as_keys())?
                            .insert(
                                "libc",
                                toml_edit::Item::from(libc_table.into_inline_table()),
                            )
                            .is_some();
                    } else {
                        inserted |= self
                            .manifest_mut()
                            .get_or_insert_nested_table(&system_requirements_table.as_keys())?
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
                .get_or_insert_nested_table(&system_requirements_table.as_keys())?
                .insert("archspec", archspec.into())
                .is_some();
        }

        Ok(inserted)
    }

    /// Sets the name of the project
    pub fn set_name(&mut self, name: &str) {
        let table = self.as_table_mut();
        if table.contains_key("project") {
            table["project"]["name"] = value(name);
        } else {
            table["workspace"]["name"] = value(name);
        }
    }

    /// Sets the description of the project
    pub fn set_description(&mut self, description: &str) {
        let table = self.as_table_mut();
        if table.contains_key("project") {
            table["project"]["description"] = value(description);
        } else {
            table["workspace"]["description"] = value(description);
        }
    }

    /// Sets the version of the project
    pub fn set_version(&mut self, version: &str) {
        let table = self.as_table_mut();
        if table.contains_key("project") {
            table["project"]["version"] = value(version);
        } else {
            table["workspace"]["version"] = value(version);
        }
    }

    /// Unsets/Sets the pixi version requirement of the project
    pub fn set_requires_pixi(&mut self, version: Option<&str>) -> Result<(), TomlError> {
        // For both 'pyproject.toml' and 'pixi.toml' manifest,
        // try and remove the dependency from pixi native tables
        let table_name = TableName::new()
            .with_prefix(self.table_prefix())
            .with_table(Some(self.detect_table_name()));

        let table = self
            .manifest_mut()
            .get_or_insert_nested_table(&table_name.as_keys())?;

        if let Some(version) = version {
            if let Some(item) = table.get_mut("requires-pixi") {
                *item = value(version);
            } else {
                table.insert("requires-pixi", value(version));
            }
        } else {
            table.remove("requires-pixi");
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// This test checks that when calling `add_dependency` with a dependency
    /// that already exists in the document the formatting and comments are
    /// preserved.
    ///
    /// This test also verifies that the name of the dependency is correctly
    /// found and kept in the situation where the user has a non-normalized name
    /// in the document (e.g. "hTTPx" vs 'HtTpX').
    #[test]
    pub fn add_dependency_retains_decoration() {
        let manifest_content = r#"[project]
name = "test"

[tool.pixi.project]
channels = []
platforms = []

[tool.pixi.dependencies]
# Hello world!
hTTPx = ">=0.28.1,<0.29" # Some comment.

# newline
"#;

        let mut document = ManifestDocument::PyProjectToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        // Overwrite the existing dependency.
        document
            .add_dependency(
                &PackageName::from_str("HtTpX").unwrap(),
                &PixiSpec::Version("0.1.*".parse().unwrap()),
                SpecType::Run,
                None,
                &FeatureName::default(),
            )
            .unwrap();

        insta::assert_snapshot!(document.to_string());
    }

    /// This test checks that when calling `add_pypi_dependency` with
    /// dependencies that already exist in different locations the
    /// formatting and comments are preserved across all PyPI dependency
    /// storage formats.
    ///
    /// This test also verifies that the name of the dependency is correctly
    /// found and kept in the situation where the user has a non-normalized name
    /// in the document (e.g. "rEquEsTs" vs 'ReQuEsTs').
    #[test]
    pub fn add_pypi_dependency_retains_decoration() {
        let manifest_content = r#"[project]
name = "test"
dependencies = [
    # Main dependency comment
    "rEquEsTs>=2.28.1,<3.0", # inline comment
]

[project.optional-dependencies]
dev = [
    # Dev dependency comment  
    "PyYaML>=6.0", # dev inline comment
]

[tool.pixi.project]
channels = []
platforms = []

[tool.pixi.pypi-dependencies]
# Table dependency comment
NumPy = ">=1.20.0" # table inline comment
"#;

        let mut document = ManifestDocument::PyProjectToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        // Update dependencies in different locations
        let requests_req = pep508_rs::Requirement::from_str("ReQuEsTs>=2.30.0").unwrap();
        document
            .add_pypi_dependency(
                &requests_req,
                None,
                None,
                &FeatureName::default(),
                None,
                Some(PypiDependencyLocation::Dependencies),
            )
            .unwrap();

        let pyyaml_req = pep508_rs::Requirement::from_str("PyYAML>=6.1.0").unwrap();
        document
            .add_pypi_dependency(
                &pyyaml_req,
                None,
                None,
                &FeatureName::from_str("dev").unwrap(),
                None,
                Some(PypiDependencyLocation::OptionalDependencies),
            )
            .unwrap();

        let numpy_req = pep508_rs::Requirement::from_str("numpy>=1.21.0").unwrap();
        document
            .add_pypi_dependency(
                &numpy_req,
                None,
                None,
                &FeatureName::default(),
                None,
                Some(crate::PypiDependencyLocation::PixiPypiDependencies),
            )
            .unwrap();

        insta::assert_snapshot!(document.to_string());
    }
}
