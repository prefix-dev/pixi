use std::fmt::{self, Display, Formatter};

use pixi_spec::PixiSpec;
use rattler_conda_types::{PackageName, Platform};
use toml_edit::{value, Array, Item, Table, Value};

use super::TomlManifest;
use crate::{consts, error::TomlError, pypi::PyPiPackageName, PyPiRequirement};
use crate::{consts::PYPROJECT_PIXI_PREFIX, FeatureName, SpecType, Task};

/// Struct that is used to access a table in `pixi.toml` or `pyproject.toml`.
pub struct TableName<'a> {
    prefix: Option<&'static str>,
    platform: Option<&'a Platform>,
    feature_name: Option<&'a FeatureName>,
    table: Option<&'a str>,
}

impl Display for TableName<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.to_toml_table_name())
    }
}

impl<'a> TableName<'a> {
    /// Create a new `TableName` with default values.
    pub fn new() -> Self {
        Self {
            prefix: None,
            platform: None,
            feature_name: None,
            table: None,
        }
    }

    /// Set the prefix of the table.
    pub fn with_prefix(mut self, prefix: Option<&'static str>) -> Self {
        self.prefix = prefix;
        self
    }

    /// Set the platform of the table.
    pub fn with_platform(mut self, platform: Option<&'a Platform>) -> Self {
        self.platform = platform;
        self
    }

    /// Set the feature name of the table.
    pub fn with_feature_name(mut self, feature_name: Option<&'a FeatureName>) -> Self {
        self.feature_name = feature_name;
        self
    }

    /// Set the optional and custom table name.
    pub fn with_table(mut self, table: Option<&'static str>) -> Self {
        self.table = table;
        self
    }
}

impl TableName<'_> {
    /// Returns the name of the table in dotted form (e.g. `table1.table2.array`).
    /// It is composed of
    /// - the 'tool.pixi' prefix if the manifest is a 'pyproject.toml' file
    /// - the feature if it is not the default feature
    /// - the platform if it is not `None`
    /// - the name of a nested TOML table if it is not `None`
    fn to_toml_table_name(&self) -> String {
        let mut parts = Vec::new();

        if self.prefix.is_some() {
            parts.push(self.prefix.unwrap());
        }

        if self
            .feature_name
            .as_ref()
            .is_some_and(|feature_name| !feature_name.is_default())
        {
            parts.push("feature");
            parts.push(
                self.feature_name
                    .as_ref()
                    .expect("we already verified")
                    .as_str(),
            );
        }
        if let Some(platform) = self.platform {
            parts.push("target");
            parts.push(platform.as_str());
        }
        if let Some(table) = self.table {
            parts.push(table);
        }
        parts.join(".")
    }
}

/// Discriminates between a 'pixi.toml' and a 'pyproject.toml' manifest
#[derive(Debug, Clone)]
pub enum ManifestSource {
    PyProjectToml(TomlManifest),
    PixiToml(TomlManifest),
}

impl fmt::Display for ManifestSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ManifestSource::PyProjectToml(document) => write!(f, "{}", document.0),
            ManifestSource::PixiToml(document) => write!(f, "{}", document.0),
        }
    }
}

impl ManifestSource {
    /// Returns a new empty pixi manifest.
    #[cfg(test)]
    fn empty_pixi() -> Self {
        ManifestSource::PixiToml(TomlManifest::default())
    }

    /// Returns a new empty pyproject manifest.
    #[cfg(test)]
    fn empty_pyproject() -> Self {
        ManifestSource::PyProjectToml(TomlManifest::default())
    }

    /// Returns the file name of the manifest
    #[cfg(test)]
    fn file_name(&self) -> &'static str {
        match self {
            ManifestSource::PyProjectToml(_) => "pyproject.toml",
            ManifestSource::PixiToml(_) => "pixi.toml",
        }
    }

    fn table_prefix(&self) -> Option<&'static str> {
        match self {
            ManifestSource::PyProjectToml(_) => Some(PYPROJECT_PIXI_PREFIX),
            ManifestSource::PixiToml(_) => None,
        }
    }

    fn manifest(&mut self) -> &mut TomlManifest {
        match self {
            ManifestSource::PyProjectToml(document) => document,
            ManifestSource::PixiToml(document) => document,
        }
    }

    /// Returns a mutable reference to the specified array either in project or
    /// feature.
    pub fn get_array_mut(
        &mut self,
        array_name: &str,
        feature_name: &FeatureName,
    ) -> Result<&mut Array, TomlError> {
        let table = match feature_name {
            FeatureName::Default => Some("project"),
            FeatureName::Named(_) => None,
        };

        let table_name = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(feature_name))
            .with_table(table);

        self.manifest()
            .get_or_insert_toml_array(table_name.to_string().as_str(), array_name)
    }

    fn as_table_mut(&mut self) -> &mut Table {
        match self {
            ManifestSource::PyProjectToml(document) => document.0.as_table_mut(),
            ManifestSource::PixiToml(document) => document.0.as_table_mut(),
        }
    }

    /// Removes a pypi dependency from the TOML manifest from
    /// native pyproject arrays and/or pixi tables as required
    ///
    /// If will be a no-op if the dependency is not found
    pub fn remove_pypi_dependency(
        &mut self,
        dep: &PyPiPackageName,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), TomlError> {
        // For 'pyproject.toml' manifest, try and remove the dependency from native
        // arrays
        let array = match self {
            ManifestSource::PyProjectToml(_) if feature_name.is_default() => {
                self.manifest().get_toml_array("project", "dependencies")?
            }
            ManifestSource::PyProjectToml(_) => self
                .manifest()
                .get_toml_array("project.optional-dependencies", &feature_name.to_string())?,
            _ => None,
        };
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
        }

        // For both 'pyproject.toml' and 'pixi.toml' manifest,
        // try and remove the dependency from pixi native tables

        let table_name = TableName::new()
            .with_prefix(self.table_prefix())
            .with_feature_name(Some(feature_name))
            .with_platform(platform.as_ref())
            .with_table(Some(consts::PYPI_DEPENDENCIES));

        self.manifest()
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

        self.manifest()
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
        // let dependency_table =
        //     self.get_or_insert_toml_table(platform, feature_name, spec_type.name())?;

        let dependency_table = TableName::new()
            .with_prefix(self.table_prefix())
            .with_platform(platform.as_ref())
            .with_feature_name(Some(feature_name))
            .with_table(Some(spec_type.name()));

        self.manifest()
            .get_or_insert_nested_table(dependency_table.to_string().as_str())
            .map(|t| t.insert(name.as_normalized(), Item::Value(spec.to_toml_value())))?;

        // dependency_table.insert(name.as_normalized(), Item::Value(spec.to_toml_value()));
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
    ) -> Result<(), TomlError> {
        match self {
            ManifestSource::PyProjectToml(_) => {
                // Pypi dependencies can be stored in different places
                // so we remove any potential dependency of the same name before adding it back
                self.remove_pypi_dependency(
                    &PyPiPackageName::from_normalized(requirement.name.clone()),
                    platform,
                    feature_name,
                )?;
                if let FeatureName::Named(name) = feature_name {
                    self.manifest()
                        .get_or_insert_toml_array("project.optional-dependencies", name)?
                        .push(requirement.to_string())
                } else {
                    self.manifest()
                        .get_or_insert_toml_array("project", "dependencies")?
                        .push(requirement.to_string())
                }
            }
            ManifestSource::PixiToml(_) => {
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

                self.manifest()
                    .get_or_insert_nested_table(dependency_table.to_string().as_str())?
                    .insert(
                        requirement.name.as_ref(),
                        Item::Value(pypi_requirement.into()),
                    );
            }
        };
        Ok(())
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

        self.manifest()
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

        self.manifest()
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

        // Get the environment table
        self.manifest()
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
            .manifest()
            .get_or_insert_nested_table(env_table.to_string().as_str())?
            .remove(name)
            .is_some())
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

#[cfg(test)]
mod tests {

    use insta::assert_snapshot;
    use rattler_conda_types::{MatchSpec, ParseStrictness::Strict};
    use rstest::rstest;

    use super::*;

    fn default_channel_config() -> rattler_conda_types::ChannelConfig {
        rattler_conda_types::ChannelConfig::default_with_root_dir(
            std::env::current_dir().expect("Could not retrieve the current directory"),
        )
    }

    #[test]
    fn test_nameless_to_toml() {
        let examples = [
            "rattler >=1",
            "conda-forge::rattler",
            "conda-forge::rattler[version=>3.0]",
            "rattler 1 *cuda",
            "rattler >=1 *cuda",
        ];

        let channel_config = default_channel_config();
        let mut table = toml_edit::DocumentMut::new();
        for example in examples {
            let spec = MatchSpec::from_str(example, Strict)
                .unwrap()
                .into_nameless()
                .1;
            let spec = PixiSpec::from_nameless_matchspec(spec, &channel_config);
            table.insert(example, Item::Value(spec.to_toml_value()));
        }
        assert_snapshot!(table);
    }

    #[test]
    fn test_get_nested_toml_table_name() {
        // Test all different options for the feature name and platform
        assert_eq!(
            "dependencies".to_string(),
            TableName::new()
                .with_feature_name(Some(&FeatureName::Default))
                .with_table(Some("dependencies"))
                .to_string()
        );

        assert_eq!(
            "target.linux-64.dependencies".to_string(),
            TableName::new()
                .with_feature_name(Some(&FeatureName::Default))
                .with_platform(Some(&Platform::Linux64))
                .with_table(Some("dependencies"))
                .to_string()
        );

        let feature_name = FeatureName::Named("test".to_string());
        assert_eq!(
            "feature.test.dependencies".to_string(),
            TableName::new()
                .with_feature_name(Some(&feature_name))
                .with_table(Some("dependencies"))
                .to_string()
        );

        assert_eq!(
            "feature.test.target.linux-64.dependencies".to_string(),
            TableName::new()
                .with_feature_name(Some(&feature_name))
                .with_platform(Some(&Platform::Linux64))
                .with_table(Some("dependencies"))
                .to_string()
        );
    }

    #[rstest]
    #[case::pixi_toml(ManifestSource::empty_pixi())]
    #[case::pyproject_toml(ManifestSource::empty_pyproject())]
    fn test_add_environment(#[case] mut source: ManifestSource) {
        source
            .add_environment("foo", Some(vec![]), None, false)
            .unwrap();
        source
            .add_environment("bar", Some(vec![String::from("default")]), None, false)
            .unwrap();
        source
            .add_environment(
                "baz",
                Some(vec![String::from("default")]),
                Some(String::from("group1")),
                false,
            )
            .unwrap();
        source
            .add_environment(
                "foobar",
                Some(vec![String::from("default")]),
                Some(String::from("group1")),
                true,
            )
            .unwrap();
        source
            .add_environment("barfoo", Some(vec![String::from("default")]), None, true)
            .unwrap();

        // Overwrite
        source
            .add_environment("bar", Some(vec![String::from("not-default")]), None, false)
            .unwrap();

        assert_snapshot!(
            format!("test_add_environment_{}", source.file_name()),
            source.to_string()
        );
    }

    #[rstest]
    #[case::pixi_toml(ManifestSource::empty_pixi())]
    #[case::pyproject_toml(ManifestSource::empty_pyproject())]
    fn test_remove_environment(#[case] mut source: ManifestSource) {
        source
            .add_environment("foo", Some(vec![String::from("default")]), None, false)
            .unwrap();
        source
            .add_environment("bar", Some(vec![String::from("default")]), None, false)
            .unwrap();
        assert!(!source.remove_environment("default").unwrap());
        source
            .add_environment("default", Some(vec![String::from("default")]), None, false)
            .unwrap();
        assert!(source.remove_environment("default").unwrap());
        assert!(source.remove_environment("foo").unwrap());
        assert_snapshot!(
            format!("test_remove_environment_{}", source.file_name()),
            source.to_string()
        );
    }
}
