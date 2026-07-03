use std::{fmt, str::FromStr, sync::Arc};

use miette::{Diagnostic, NamedSource};
use pixi_consts::consts;
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use rattler_conda_types::PackageName;
use thiserror::Error;
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use crate::{
    FeatureName, ManifestKind, ManifestProvenance, PixiPlatform, PixiPlatformName,
    PypiDependencyLocation, SpecType, TargetSelector, Task, TomlError,
    manifests::table_name::TableName, toml::TomlDocument, utils::WithSourceCode,
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
            ManifestDocument::PyProjectToml(document) => write!(f, "{document}"),
            ManifestDocument::PixiToml(document) => write!(f, "{document}"),
            ManifestDocument::MojoProjectToml(document) => write!(f, "{document}"),
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
        [tool.pixi.workspace]
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
        platform: Option<PixiPlatformName>,
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
            .with_target(platform.map(TargetSelector::Platform))
            .with_table(Some(consts::PYPI_DEPENDENCIES));

        let item = self
            .manifest_mut()
            .get_or_insert_nested_item(&table_name.as_keys())?;
        // Look up the existing key so a non-normalized spelling in the
        // document (e.g. "PyYAML" vs "pyyaml") is found as well.
        let key = existing_pypi_key(item, dep.as_normalized())
            .unwrap_or_else(|| dep.as_source().to_string());
        pixi_toml_edit::remove_entry(item, &key).map_err(|_| {
            TomlError::table_error(consts::PYPI_DEPENDENCIES, &table_name.to_string())
        })?;
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
            pixi_toml_edit::retain_array_elements(array, |x| {
                // Entries that are not valid pep508 requirements cannot be
                // the dependency we are looking for -- leave them alone.
                let Some(req) = x
                    .as_str()
                    .and_then(|s| s.parse::<pep508_rs::Requirement>().ok())
                else {
                    return true;
                };
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
        platform: Option<PixiPlatformName>,
        feature_name: &FeatureName,
    ) -> Result<(), TomlError> {
        let table_name = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(feature_name))
            .with_target(platform.map(TargetSelector::Platform))
            .with_table(Some(spec_type.name()));

        let item = self
            .manifest_mut()
            .get_or_insert_nested_item(&table_name.as_keys())?;
        // Look up the existing key so a non-normalized spelling in the
        // document (e.g. "hTTPx" vs "httpx") is found as well.
        let key = existing_conda_key(item, dep).unwrap_or_else(|| dep.as_source().to_string());
        pixi_toml_edit::remove_entry(item, &key)
            .map_err(|_| TomlError::table_error(spec_type.name(), &table_name.to_string()))?;
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
        target: Option<&TargetSelector>,
        feature_name: &FeatureName,
    ) -> Result<(), TomlError> {
        let dependency_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_target(target.cloned())
            .with_feature_name(Some(feature_name))
            .with_table(Some(spec_type.name()));

        let item = self
            .manifest_mut()
            .get_or_insert_nested_item(&dependency_table.as_keys())?;

        // Look up the existing key so a non-normalized spelling in the
        // document (e.g. "hTTPx" vs "httpx") is overwritten in place instead
        // of inserted a second time.
        let existing_key = existing_conda_key(item, name);
        let key = existing_key.as_deref().unwrap_or(name.as_normalized());

        pixi_toml_edit::upsert_entry(item, key, spec.to_toml_value())
            .map_err(|_| TomlError::table_error(spec_type.name(), &dependency_table.to_string()))?;

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
        target: Option<&TargetSelector>,
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
            || target.is_some()
            || editable.is_some_and(|e| e)
        {
            let mut pypi_requirement = match pixi_requirement {
                Some(existing) => existing.update_requirement(requirement)?,
                None => PixiPypiSpec::try_from(requirement.clone()).map_err(Box::new)?,
            };
            if let Some(editable) = editable {
                pypi_requirement.set_editable(editable);
            }

            let dependency_table_name = TableName::new()
                .with_prefix(self.table_prefix())
                .with_target(target.cloned())
                .with_feature_name(Some(feature_name))
                .with_table(Some(consts::PYPI_DEPENDENCIES));

            let item = self
                .manifest_mut()
                .get_or_insert_nested_item(&dependency_table_name.as_keys())?;
            upsert_pypi_requirement(item, &requirement.name, Value::from(pypi_requirement))
                .map_err(|_| {
                    TomlError::table_error(
                        consts::PYPI_DEPENDENCIES,
                        &dependency_table_name.to_string(),
                    )
                })?;

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
        //   - an existing [tool.pixi.feature.<feature>.pypi-dependencies] table exists
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
                pixi_toml_edit::push_array_element(array, requirement.to_string().into());
            }
            Ok(())
        };

        // Reuse existing feature pypi-dependencies table if present
        let has_existing_pixi_pypi_deps = if !feature_name.is_default() && location.is_none() {
            let table_name = TableName::new()
                .with_prefix(self.table_prefix())
                .with_feature_name(Some(feature_name))
                .with_table(Some(consts::PYPI_DEPENDENCIES));
            self.manifest()
                .get_nested_table(&table_name.as_keys())
                .is_ok()
        } else {
            false
        };

        if has_existing_pixi_pypi_deps {
            let pypi_requirement = PixiPypiSpec::try_from(requirement.clone()).map_err(Box::new)?;

            let dependency_table_name = TableName::new()
                .with_prefix(self.table_prefix())
                .with_feature_name(Some(feature_name))
                .with_table(Some(consts::PYPI_DEPENDENCIES));

            let item = self
                .manifest_mut()
                .get_or_insert_nested_item(&dependency_table_name.as_keys())?;
            upsert_pypi_requirement(item, &requirement.name, Value::from(pypi_requirement))
                .map_err(|_| {
                    TomlError::table_error(
                        consts::PYPI_DEPENDENCIES,
                        &dependency_table_name.to_string(),
                    )
                })?;
        } else if feature_name.is_default()
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
        target: Option<&TargetSelector>,
        feature_name: &FeatureName,
    ) -> Option<PypiDependencyLocation> {
        // For both 'pyproject.toml' and 'pixi.toml' manifest,
        // try and to get `pypi-dependency`
        let table_name = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(feature_name))
            .with_target(target.cloned())
            .with_table(Some(consts::PYPI_DEPENDENCIES));

        let pypi_dependency_table = self.manifest().get_nested_table(&table_name.as_keys()).ok();

        // Compare under pep508 name normalization so a differently spelled
        // key in the document (e.g. "PyYAML" vs "pyyaml") is found as well.
        if pypi_dependency_table.is_some_and(|table| {
            table.iter().any(|(key, _)| {
                pep508_rs::PackageName::from_str(key)
                    .is_ok_and(|existing| existing == *package_name.as_normalized())
            })
        }) {
            return Some(PypiDependencyLocation::PixiPypiDependencies);
        }

        // Whether `package_name` is present in the given PEP 508 requirement
        // array. Membership (not just the array's existence) decides the
        // location, so a package is reported in the section it truly belongs to.
        let array_contains_package = |keys: &[&str], array_name: &str| -> bool {
            let Ok(Some(array)) = self.manifest().get_toml_array(keys, array_name) else {
                return false;
            };
            array.iter().any(|item| {
                item.as_str()
                    .and_then(|s| s.parse::<pep508_rs::Requirement>().ok())
                    .is_some_and(|req| req.name == *package_name.as_normalized())
            })
        };

        if array_contains_package(&["project"], "dependencies") {
            return Some(PypiDependencyLocation::Dependencies);
        }
        let name = feature_name.to_string();

        if array_contains_package(&["project", "optional-dependencies"], &name) {
            return Some(PypiDependencyLocation::OptionalDependencies);
        }

        if array_contains_package(&["dependency-groups"], &name) {
            return Some(PypiDependencyLocation::DependencyGroups);
        }

        None
    }

    /// Removes a task from the TOML manifest
    pub fn remove_task(
        &mut self,
        name: &str,
        platform: Option<&PixiPlatform>,
        feature_name: &FeatureName,
    ) -> Result<(), TomlError> {
        // Get the task table either from the target platform or the default tasks.
        // If it does not exist in TOML, consider this ok as we want to remove it
        // anyways
        let task_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_target(platform.map(PixiPlatform::as_target_selector))
            .with_feature_name(Some(feature_name))
            .with_table(Some("tasks"));

        let item = self
            .manifest_mut()
            .get_or_insert_nested_item(&task_table.as_keys())?;
        pixi_toml_edit::remove_entry(item, name)
            .map_err(|_| TomlError::table_error("tasks", &task_table.to_string()))?;

        Ok(())
    }

    /// Adds a task to the TOML manifest
    pub fn add_task<'a>(
        &'a mut self,
        name: &'a str,
        task: Task,
        platform: Option<&'a PixiPlatform>,
        feature_name: &'a FeatureName,
    ) -> Result<(), TomlError> {
        // Get the task table either from the target platform or the default tasks.
        let task_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_target(platform.map(PixiPlatform::as_target_selector))
            .with_feature_name(Some(feature_name))
            .with_table(Some("tasks"));

        let item = self
            .manifest_mut()
            .get_or_insert_nested_item(&task_table.as_keys())?;
        match Item::from(task) {
            Item::Value(value) => pixi_toml_edit::upsert_entry(item, name, value)
                .map_err(|_| TomlError::table_error("tasks", &task_table.to_string()))?,
            task_item => {
                item.as_table_like_mut()
                    .ok_or_else(|| TomlError::table_error("tasks", &task_table.to_string()))?
                    .insert(name, task_item);
            }
        }

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
        // Construct the TOML value
        let value = if solve_group.is_some() || no_default_features {
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
            Value::from(table)
        } else {
            Value::Array(Array::from_iter(features.into_iter().flatten()))
        };

        let env_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(&FeatureName::DEFAULT))
            .with_table(Some("environments"));

        // Insert into the environment table
        let item = self
            .manifest_mut()
            .get_or_insert_nested_item(&env_table.as_keys())?;
        pixi_toml_edit::upsert_entry(item, &name.into(), value)
            .map_err(|_| TomlError::table_error("environments", &env_table.to_string()))?;

        Ok(())
    }

    /// Removes an environment from the manifest. Returns `true` if the
    /// environment was removed.
    pub fn remove_environment(&mut self, name: &str) -> Result<bool, TomlError> {
        let env_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(&FeatureName::DEFAULT))
            .with_table(Some("environments"));

        let item = self
            .manifest_mut()
            .get_or_insert_nested_item(&env_table.as_keys())?;
        Ok(pixi_toml_edit::remove_entry(item, name)
            .map_err(|_| TomlError::table_error("environments", &env_table.to_string()))?
            .is_some())
    }

    /// Removes a feature from the manifest. Returns `true` if the feature was
    /// removed.
    pub fn remove_feature(&mut self, feature_name: &FeatureName) -> Result<bool, TomlError> {
        let table_name = TableName::new()
            .with_prefix(self.table_prefix())
            .with_table(Some("feature"));

        let feature_table = self
            .manifest_mut()
            .get_or_insert_nested_table(&table_name.as_keys())?;

        Ok(feature_table.remove(feature_name.as_str()).is_some())
    }

    /// Remove the `[system-requirements]` table from the document. When
    /// `feature_name` is `Some` and refers to a named feature, removes
    /// `[feature.X.system-requirements]`; otherwise removes the workspace-level
    /// table. No-op if the table doesn't exist.
    pub fn remove_system_requirements_section(
        &mut self,
        feature_name: Option<&FeatureName>,
    ) -> Result<(), TomlError> {
        let table_name = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(feature_name);
        let keys = table_name.as_keys();
        // Don't materialise the parent table just to remove a child from it: a
        // missing parent (e.g. `[feature.X]`) means there's nothing to remove.
        if self.manifest_mut().get_nested_table(&keys).is_err() {
            return Ok(());
        }
        self.manifest_mut()
            .get_or_insert_nested_table(&keys)?
            .remove(consts::SYSTEM_REQUIREMENTS);
        Ok(())
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

/// The key under which a conda package is stored in the table-like item,
/// honoring conda package name normalization so a differently spelled key
/// (e.g. "hTTPx" vs "httpx") is found.
fn existing_conda_key(item: &Item, name: &PackageName) -> Option<String> {
    pixi_toml_edit::find_table_key(item, |key| {
        PackageName::from_str(key).is_ok_and(|existing| existing == *name)
    })
}

/// The key under which a pypi package is stored in the table-like item,
/// honoring pep508 name normalization so a differently spelled key (e.g.
/// "PyYAML" vs "pyyaml") is found.
fn existing_pypi_key(item: &Item, name: &pep508_rs::PackageName) -> Option<String> {
    pixi_toml_edit::find_table_key(item, |key| {
        pep508_rs::PackageName::from_str(key).is_ok_and(|existing| existing == *name)
    })
}

/// Inserts or overwrites a pypi requirement in a table-like item, looking up
/// the existing entry under pep508 name normalization so a differently
/// spelled key (e.g. "PyYAML" vs "pyyaml") is overwritten in place.
fn upsert_pypi_requirement(
    item: &mut Item,
    name: &pep508_rs::PackageName,
    value: Value,
) -> Result<(), pixi_toml_edit::NotATableError> {
    let existing_key = existing_pypi_key(item, name);
    let key = existing_key.as_deref().unwrap_or(name.as_ref());
    pixi_toml_edit::upsert_entry(item, key, value)
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

[tool.pixi.workspace]
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
                &PixiSpec::from("0.1.*".parse::<rattler_conda_types::VersionSpec>().unwrap()),
                SpecType::Run,
                None,
                &FeatureName::default(),
            )
            .unwrap();

        insta::assert_snapshot!(document.to_string());
    }

    /// Adding a dependency to a table written as a TOML 1.1 multiline inline
    /// table puts the new entry on its own line, mimicking the existing
    /// entries.
    #[test]
    pub fn add_dependency_to_multiline_inline_table() {
        let manifest_content = r#"[workspace]
channels = ["conda-forge"]
name = "test"
platforms = ["linux-64"]

[feature.test]
dependencies = {
    numpy = "*",
}

[environments]
test = ["test"]
"#;

        let mut document = ManifestDocument::PixiToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        document
            .add_dependency(
                &PackageName::from_str("pydantic").unwrap(),
                &PixiSpec::from(
                    ">=2.12.5,<3"
                        .parse::<rattler_conda_types::VersionSpec>()
                        .unwrap(),
                ),
                SpecType::Run,
                None,
                &FeatureName::from("test"),
            )
            .unwrap();

        insta::assert_snapshot!(document.to_string(), @r#"
        [workspace]
        channels = ["conda-forge"]
        name = "test"
        platforms = ["linux-64"]

        [feature.test]
        dependencies = {
            numpy = "*",
            pydantic = ">=2.12.5,<3",
        }

        [environments]
        test = ["test"]
        "#);
    }

    /// Removing a dependency from a TOML 1.1 multiline inline table removes
    /// the whole line, and the closing brace stays on its own line.
    #[test]
    pub fn remove_dependency_from_multiline_inline_table() {
        let manifest_content = r#"[workspace]
channels = ["conda-forge"]
name = "test"
platforms = ["linux-64"]

[feature.test]
dependencies = {
    numpy = "*",
    pydantic = ">=2.12.5,<3",
}

[environments]
test = ["test"]
"#;

        let mut document = ManifestDocument::PixiToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        document
            .remove_dependency(
                &PackageName::from_str("pydantic").unwrap(),
                SpecType::Run,
                None,
                &FeatureName::from("test"),
            )
            .unwrap();

        insta::assert_snapshot!(document.to_string(), @r#"
        [workspace]
        channels = ["conda-forge"]
        name = "test"
        platforms = ["linux-64"]

        [feature.test]
        dependencies = {
            numpy = "*",
        }

        [environments]
        test = ["test"]
        "#);
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

[tool.pixi.workspace]
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

    /// Reproduction for https://github.com/prefix-dev/pixi/issues/6239
    ///
    /// When a pyproject.toml has both a `[project.dependencies]` array and a
    /// `[dependency-groups]` section, `pypi_dependency_location` must report
    /// that a package belonging to a dependency group lives in
    /// `DependencyGroups` (not `Dependencies`). Otherwise `pixi upgrade`
    /// writes the upgraded constraint into `[project.dependencies]`.
    #[test]
    pub fn repro_6239_dependency_group_location() {
        let manifest_content = r#"[project]
name = "test"
dependencies = [
    "requests>=2.28.1,<3.0",
]

[dependency-groups]
dev = [
    "pytest>=8.0,<9",
    "ruff>=0.5,<0.6",
]

[tool.pixi.workspace]
channels = ["conda-forge"]
platforms = ["linux-64"]

[tool.pixi.environments]
dev = ["dev"]
"#;

        let document = ManifestDocument::PyProjectToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        let location_name = |pkg: &str, feature: &FeatureName| -> &'static str {
            let name = PypiPackageName::from_str(pkg).unwrap();
            match document.pypi_dependency_location(&name, None, feature) {
                Some(PypiDependencyLocation::PixiPypiDependencies) => "PixiPypiDependencies",
                Some(PypiDependencyLocation::Dependencies) => "Dependencies",
                Some(PypiDependencyLocation::OptionalDependencies) => "OptionalDependencies",
                Some(PypiDependencyLocation::DependencyGroups) => "DependencyGroups",
                None => "None",
            }
        };

        let dev_feature = FeatureName::from_str("dev").unwrap();

        // `pytest` is defined in `[dependency-groups].dev`, so it must be
        // reported as living in the dependency-groups section, not in
        // `[project.dependencies]`.
        assert_eq!(
            location_name("pytest", &dev_feature),
            "DependencyGroups",
            "pytest belongs to [dependency-groups].dev"
        );

        // A package genuinely listed in `[project.dependencies]` must still be
        // reported there.
        assert_eq!(
            location_name("requests", &FeatureName::default()),
            "Dependencies",
            "requests belongs to [project.dependencies]"
        );
    }

    /// This test checks that removing a pypi dependency
    /// uses the same source name as the one used to add it.
    #[test]
    pub fn remove_pypi_dependency() {
        let manifest_content = r#"
[project]
name = "pixi-demo"
requires-python = ">= 3.11"
version = "0.1.0"

[tool.pixi.workspace]
channels = ["conda-forge"]
platforms = ["osx-arm64"]

[tool.pixi.pypi-dependencies]
pixi_demo = { path = ".", editable = true }
"#;

        let mut document = ManifestDocument::PyProjectToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        let pypi_name = PypiPackageName::from_str("pixi_demo").unwrap();

        document
            .remove_pypi_dependency(&pypi_name, None, &FeatureName::default())
            .unwrap();

        insta::assert_snapshot!(document.to_string());
    }

    /// This test checks that removing a pypi dependency with a differently
    /// spelled name removes it as well: pep508 names compare under
    /// normalization, so "pixi-demo" and "pixi_demo" are the same package.
    #[test]
    pub fn remove_pypi_dependency_with_different_name() {
        let manifest_content = r#"
[project]
name = "pixi-demo"
requires-python = ">= 3.11"
version = "0.1.0"

[tool.pixi.workspace]
channels = ["conda-forge"]
platforms = ["osx-arm64"]

[tool.pixi.pypi-dependencies]
pixi_demo = { path = ".", editable = true }
"#;

        let mut document = ManifestDocument::PyProjectToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        // Important bit here that is different from `remove_pypi_dependency` test:
        // using a dash instead of an underscore
        let pypi_name = PypiPackageName::from_str("pixi-demo").unwrap();

        document
            .remove_pypi_dependency(&pypi_name, None, &FeatureName::default())
            .unwrap();

        insta::assert_snapshot!(document.to_string());
    }

    /// This test checks that removing a feature removes all its subtables.
    #[test]
    pub fn remove_feature_pixi_toml() {
        let manifest_content = r#"
[workspace]
name = "test"
channels = ["conda-forge"]
platforms = ["linux-64"]

[feature.test]
channels = ["test-channel"]

[feature.test.dependencies]
some-package = "*"

[feature.test.target.linux-64.dependencies]
linux-package = "*"

[feature.other]
channels = ["other-channel"]
"#;

        let mut document = ManifestDocument::PixiToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        // Remove the feature
        let removed = document
            .remove_feature(&FeatureName::from_str("test").unwrap())
            .unwrap();
        assert!(removed);

        // Verify the feature and all its subtables are removed
        let result = document.to_string();
        assert!(!result.contains("[feature.test]"));
        assert!(!result.contains("some-package"));
        assert!(!result.contains("linux-package"));

        // Verify other feature is still there
        assert!(result.contains("[feature.other]"));

        // Remove non-existent feature should return false
        let removed = document
            .remove_feature(&FeatureName::from_str("nonexistent").unwrap())
            .unwrap();
        assert!(!removed);
    }

    /// This test checks that removing a feature works in pyproject.toml.
    #[test]
    pub fn remove_feature_pyproject_toml() {
        let manifest_content = r#"
[project]
name = "test"

[tool.pixi.workspace]
channels = ["conda-forge"]
platforms = ["linux-64"]

[tool.pixi.feature.test]
channels = ["test-channel"]

[tool.pixi.feature.test.dependencies]
some-package = "*"

[tool.pixi.feature.other]
channels = ["other-channel"]
"#;

        let mut document = ManifestDocument::PyProjectToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        // Remove the feature
        let removed = document
            .remove_feature(&FeatureName::from_str("test").unwrap())
            .unwrap();
        assert!(removed);

        // Verify the feature and all its subtables are removed
        let result = document.to_string();
        assert!(!result.contains("[tool.pixi.feature.test]"));
        assert!(!result.contains("some-package"));

        // Verify other feature is still there
        assert!(result.contains("[tool.pixi.feature.other]"));
    }

    /// Regression test for https://github.com/prefix-dev/pixi/issues/5492
    #[test]
    pub fn add_pypi_dependency_reuses_existing_feature_table() {
        let manifest_content = r#"[project]
name = "test"

[tool.pixi.workspace]
channels = []
platforms = []

[tool.pixi.feature.cuda.pypi-dependencies]
torch = ">=2.0.0"
"#;

        let mut document = ManifestDocument::PyProjectToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        let numpy_req = pep508_rs::Requirement::from_str("numpy>=1.20.0").unwrap();
        document
            .add_pypi_dependency(
                &numpy_req,
                None,
                None,
                &FeatureName::from_str("cuda").unwrap(),
                None,
                None,
            )
            .unwrap();

        let result = document.to_string();

        assert!(result.contains("[tool.pixi.feature.cuda.pypi-dependencies]"));
        assert!(result.contains("numpy"));
        assert!(!result.contains("[dependency-groups]"));

        insta::assert_snapshot!(result);
    }

    #[test]
    pub fn add_pypi_dependency_creates_dependency_groups_when_no_existing_table() {
        let manifest_content = r#"[project]
name = "test"

[tool.pixi.workspace]
channels = []
platforms = []
"#;

        let mut document = ManifestDocument::PyProjectToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        let numpy_req = pep508_rs::Requirement::from_str("numpy>=1.20.0").unwrap();
        document
            .add_pypi_dependency(
                &numpy_req,
                None,
                None,
                &FeatureName::from_str("cuda").unwrap(),
                None,
                None,
            )
            .unwrap();

        let result = document.to_string();

        assert!(result.contains("[dependency-groups]"));
        assert!(result.contains("cuda"));
        assert!(result.contains("numpy"));

        insta::assert_snapshot!(result);
    }

    /// Removing a dependency must find the entry in the document even when
    /// the document spells the name differently than the user typed it:
    /// conda package names compare case-insensitively.
    #[test]
    pub fn remove_dependency_with_non_normalized_name_in_document() {
        let manifest_content = r#"[workspace]
channels = ["conda-forge"]
name = "test"
platforms = ["linux-64"]

[dependencies]
hTTPx = "*"
numpy = "*"
"#;

        let mut document = ManifestDocument::PixiToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        document
            .remove_dependency(
                &PackageName::from_str("httpx").unwrap(),
                SpecType::Run,
                None,
                &FeatureName::default(),
            )
            .unwrap();

        insta::assert_snapshot!(document.to_string(), @r#"
        [workspace]
        channels = ["conda-forge"]
        name = "test"
        platforms = ["linux-64"]

        [dependencies]
        numpy = "*"
        "#);
    }

    /// Removing a dependency from a regular `[dependencies]` table keeps
    /// standalone comment lines above the removed entry.
    #[test]
    pub fn remove_dependency_keeps_standalone_comment_in_regular_table() {
        let manifest_content = r#"[workspace]
channels = ["conda-forge"]
name = "test"
platforms = ["linux-64"]

[dependencies]
# core scientific stack
numpy = "*"
scipy = "*"
"#;

        let mut document = ManifestDocument::PixiToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        document
            .remove_dependency(
                &PackageName::from_str("numpy").unwrap(),
                SpecType::Run,
                None,
                &FeatureName::default(),
            )
            .unwrap();

        insta::assert_snapshot!(document.to_string(), @r#"
        [workspace]
        channels = ["conda-forge"]
        name = "test"
        platforms = ["linux-64"]

        [dependencies]
        # core scientific stack
        scipy = "*"
        "#);
    }

    /// Adding a complex task to a tasks table written as a TOML 1.1
    /// multiline inline table must keep the document valid and put the task
    /// on its own line like the simple tasks.
    #[test]
    pub fn add_complex_task_to_multiline_inline_tasks_table() {
        let manifest_content = r#"[workspace]
channels = ["conda-forge"]
name = "test"
platforms = ["linux-64"]

[feature.dev]
tasks = {
    fmt = "cargo fmt",
}

[environments]
dev = ["dev"]
"#;

        let mut document = ManifestDocument::PixiToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        document
            .add_task(
                "lint",
                Task::Execute(Box::new(crate::task::Execute {
                    cmd: crate::task::CmdArgs::Single("cargo clippy".into()),
                    inputs: None,
                    outputs: None,
                    depends_on: Vec::new(),
                    cwd: None,
                    env: None,
                    default_environment: None,
                    description: None,
                    clean_env: false,
                    args: None,
                })),
                None,
                &FeatureName::from("dev"),
            )
            .unwrap();

        let result = document.to_string();
        // The document must stay parseable no matter the formatting.
        DocumentMut::from_str(&result).expect("edited manifest must stay valid TOML");
        insta::assert_snapshot!(result, @r#"
        [workspace]
        channels = ["conda-forge"]
        name = "test"
        platforms = ["linux-64"]

        [feature.dev]
        tasks = {
            fmt = "cargo fmt",
            lint = { cmd = "cargo clippy" },
        }

        [environments]
        dev = ["dev"]
        "#);
    }

    /// Adding a dependency when the feature writes its dependencies with a
    /// dotted key must not lose the existing entry or produce invalid TOML.
    #[test]
    pub fn add_dependency_to_dotted_key_dependencies() {
        let manifest_content = r#"[workspace]
channels = ["conda-forge"]
name = "test"
platforms = ["linux-64"]

[feature.test]
dependencies.numpy = "*"

[environments]
test = ["test"]
"#;

        let mut document = ManifestDocument::PixiToml(TomlDocument::new(
            DocumentMut::from_str(manifest_content).unwrap(),
        ));

        document
            .add_dependency(
                &PackageName::from_str("pydantic").unwrap(),
                &PixiSpec::from(
                    ">=2,<3"
                        .parse::<rattler_conda_types::VersionSpec>()
                        .unwrap(),
                ),
                SpecType::Run,
                None,
                &FeatureName::from("test"),
            )
            .unwrap();

        let result = document.to_string();
        let reparsed =
            DocumentMut::from_str(&result).expect("edited manifest must stay valid TOML");
        let deps = reparsed["feature"]["test"]["dependencies"]
            .as_table_like()
            .expect("dependencies must still be table-like");
        assert!(deps.get("numpy").is_some(), "numpy was lost:\n{result}");
        assert!(
            deps.get("pydantic").is_some(),
            "pydantic missing:\n{result}"
        );
    }
}
