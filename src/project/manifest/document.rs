use std::{fmt, str::FromStr};

use rattler_conda_types::{NamelessMatchSpec, PackageName, Platform};
use toml_edit::{value, Array, InlineTable, Item, Table, Value};

use super::{error::TomlError, python::PyPiPackageName, PyPiRequirement};
use crate::{consts, util::default_channel_config, FeatureName, SpecType, Task};

const PYPROJECT_PIXI_PREFIX: &str = "tool.pixi";

/// Discriminates between a 'pixi.toml' and a 'pyproject.toml' manifest
#[derive(Debug, Clone)]
pub enum ManifestSource {
    PyProjectToml(toml_edit::DocumentMut),
    PixiToml(toml_edit::DocumentMut),
}

impl fmt::Display for ManifestSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ManifestSource::PyProjectToml(document) => write!(f, "{}", document),
            ManifestSource::PixiToml(document) => write!(f, "{}", document),
        }
    }
}

impl ManifestSource {
    /// Returns a new empty pixi manifest.
    #[cfg(test)]
    fn empty_pixi() -> Self {
        ManifestSource::PixiToml(toml_edit::DocumentMut::new())
    }

    /// Returns a new empty pyproject manifest.
    #[cfg(test)]
    fn empty_pyproject() -> Self {
        ManifestSource::PyProjectToml(toml_edit::DocumentMut::new())
    }

    /// Returns the file name of the manifest
    #[cfg(test)]
    fn file_name(&self) -> &'static str {
        match self {
            ManifestSource::PyProjectToml(_) => "pyproject.toml",
            ManifestSource::PixiToml(_) => "pixi.toml",
        }
    }

    /// Returns the a nested path. It is composed of
    /// - the 'tool.pixi' prefix if the manifest is a 'pyproject.toml' file
    /// - the feature if it is not the default feature
    /// - the platform if it is not `None`
    /// - the name of a nested TOML table if it is not `None`
    fn get_nested_toml_table_name(
        &self,
        feature_name: &FeatureName,
        platform: Option<Platform>,
        table: Option<&str>,
    ) -> String {
        let mut parts = Vec::new();
        if let ManifestSource::PyProjectToml(_) = self {
            parts.push(PYPROJECT_PIXI_PREFIX);
        }
        if !feature_name.is_default() {
            parts.push("feature");
            parts.push(feature_name.as_str());
        }
        if let Some(platform) = platform {
            parts.push("target");
            parts.push(platform.as_str());
        }
        if let Some(table) = table {
            parts.push(table);
        }
        parts.join(".")
    }

    /// Retrieve a mutable reference to a target table `table_name`
    /// for a specific platform and feature.
    /// If the table is not found, it is inserted into the document.
    fn get_or_insert_toml_table<'a>(
        &'a mut self,
        platform: Option<Platform>,
        feature: &FeatureName,
        table_name: &str,
    ) -> Result<&'a mut Table, TomlError> {
        let table_name: String =
            self.get_nested_toml_table_name(feature, platform, Some(table_name));
        self.get_or_insert_nested_table(&table_name)
    }

    /// Retrieve a mutable reference to a target table `table_name`
    /// in dotted form (e.g. `table1.table2`) from the root of the document.
    /// If the table is not found, it is inserted into the document.
    fn get_or_insert_nested_table<'a>(
        &'a mut self,
        table_name: &str,
    ) -> Result<&'a mut Table, TomlError> {
        let parts: Vec<&str> = table_name.split('.').collect();

        let mut current_table = self.as_table_mut();

        for part in parts {
            let entry = current_table.entry(part);
            let item = entry.or_insert(Item::Table(Table::new()));
            current_table = item
                .as_table_mut()
                .ok_or_else(|| TomlError::table_error(part, table_name))?;
            // Avoid creating empty tables
            current_table.set_implicit(true);
        }
        Ok(current_table)
    }

    /// Retrieves a mutable reference to a target array `array_name`
    /// in table `table_name` in dotted form (e.g. `table1.table2.array`).
    ///
    /// If the array is not found, it is inserted into the document.
    fn get_or_insert_toml_array<'a>(
        &'a mut self,
        table_name: &str,
        array_name: &str,
    ) -> Result<&'a mut Array, TomlError> {
        self.get_or_insert_nested_table(table_name)?
            .entry(array_name)
            .or_insert(Item::Value(Value::Array(Array::new())))
            .as_array_mut()
            .ok_or_else(|| TomlError::array_error(array_name, table_name))
    }

    /// Retrieves a mutable reference to a target array `array_name`
    /// in table `table_name` in dotted form (e.g. `table1.table2.array`).
    ///
    /// If the array is not found, returns None.
    fn get_toml_array<'a>(
        &'a mut self,
        table_name: &str,
        array_name: &str,
    ) -> Result<Option<&'a mut Array>, TomlError> {
        let array = self
            .get_or_insert_nested_table(table_name)?
            .get_mut(array_name)
            .and_then(|a| a.as_array_mut());
        Ok(array)
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
        let table_name = self.get_nested_toml_table_name(feature_name, None, table);
        self.get_or_insert_toml_array(&table_name, array_name)
    }

    fn as_table_mut(&mut self) -> &mut Table {
        match self {
            ManifestSource::PyProjectToml(document) => document.as_table_mut(),
            ManifestSource::PixiToml(document) => document.as_table_mut(),
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
                self.get_toml_array("project", "dependencies")?
            }
            ManifestSource::PyProjectToml(_) => {
                self.get_toml_array("project.optional-dependencies", &feature_name.to_string())?
            }
            _ => None,
        };
        if let Some(array) = array {
            array.retain(|x| {
                let name = PyPiPackageName::from_normalized(
                    pep508_rs::Requirement::from_str(x.as_str().unwrap_or(""))
                        .expect("should be a valid pep508 dependency")
                        .name,
                );
                name != *dep
            });
        }

        // For both 'pyproject.toml' and 'pixi.toml' manifest,
        // try and remove the dependency from pixi native tables
        self.get_or_insert_toml_table(platform, feature_name, consts::PYPI_DEPENDENCIES)
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
        self.get_or_insert_toml_table(platform, feature_name, spec_type.name())
            .map(|t| t.remove(dep.as_source()))?;
        Ok(())
    }

    /// Adds a conda dependency to the TOML manifest
    ///
    /// If a dependency with the same name already exists, it will be replaced.
    pub fn add_dependency(
        &mut self,
        name: &PackageName,
        spec: &NamelessMatchSpec,
        spec_type: SpecType,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), TomlError> {
        let dependency_table =
            self.get_or_insert_toml_table(platform, feature_name, spec_type.name())?;
        dependency_table.insert(
            name.as_normalized(),
            Item::Value(nameless_match_spec_to_toml(spec)),
        );
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
                    self.get_or_insert_toml_array("project.optional-dependencies", name)?
                        .push(requirement.to_string())
                } else {
                    self.get_or_insert_toml_array("project", "dependencies")?
                        .push(requirement.to_string())
                }
            }
            ManifestSource::PixiToml(_) => {
                self.get_or_insert_toml_table(platform, feature_name, consts::PYPI_DEPENDENCIES)?
                    .insert(
                        requirement.name.as_ref(),
                        Item::Value(PyPiRequirement::from(requirement.clone()).into()),
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
        self.get_or_insert_toml_table(platform, feature_name, "tasks")?
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
        self.get_or_insert_toml_table(platform, feature_name, "tasks")?
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
                table.insert("no-default-features", true.into());
            }
            Item::Value(table.into())
        } else {
            Item::Value(Value::Array(Array::from_iter(
                features.into_iter().flatten(),
            )))
        };

        // Get the environment table
        self.get_or_insert_toml_table(None, &FeatureName::Default, "environments")?
            .insert(&name.into(), item);

        Ok(())
    }

    /// Removes an environment from the manifest. Returns `true` if the
    /// environment was removed.
    pub fn remove_environment(&mut self, name: &str) -> Result<bool, TomlError> {
        Ok(self
            .get_or_insert_toml_table(None, &FeatureName::Default, "environments")?
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

/// Given a nameless matchspec convert it into a TOML value. If the spec only
/// contains a version a string is returned, otherwise an entire table is
/// constructed.
fn nameless_match_spec_to_toml(spec: &NamelessMatchSpec) -> Value {
    match spec {
        NamelessMatchSpec {
            version,
            build: None,
            build_number: None,
            file_name: None,
            channel: None,
            subdir: None,
            namespace: None,
            md5: None,
            sha256: None,
        } => {
            // No other fields besides the version was specified, so we can just return the
            // version as a string.
            version
                .as_ref()
                .map_or_else(|| String::from("*"), |v| v.to_string())
                .into()
        }
        NamelessMatchSpec {
            version,
            build,
            build_number,
            file_name,
            channel,
            subdir,
            namespace,
            md5,
            sha256,
        } => {
            let mut table = InlineTable::new();
            table.insert(
                "version",
                version
                    .as_ref()
                    .map_or_else(|| String::from("*"), |v| v.to_string())
                    .into(),
            );
            if let Some(build) = build {
                table.insert("build", build.to_string().into());
            }
            if let Some(build_number) = build_number {
                table.insert("build_number", build_number.to_string().into());
            }
            if let Some(file_name) = file_name {
                table.insert("file_name", file_name.to_string().into());
            }
            if let Some(channel) = channel {
                table.insert(
                    "channel",
                    default_channel_config()
                        .canonical_name(channel.base_url())
                        .as_str()
                        .into(),
                );
            }
            if let Some(subdir) = subdir {
                table.insert("subdir", subdir.to_string().into());
            }
            if let Some(namespace) = namespace {
                table.insert("namespace", namespace.to_string().into());
            }
            if let Some(md5) = md5 {
                table.insert("md5", format!("{:x}", md5).into());
            }
            if let Some(sha256) = sha256 {
                table.insert("sha256", format!("{:x}", sha256).into());
            }
            table.into()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use insta::assert_snapshot;
    use rattler_conda_types::{MatchSpec, ParseStrictness::Strict};
    use rstest::rstest;

    use super::*;
    use crate::project::manifest::Manifest;

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = ["linux-64", "win-64", "osx-64"]
        "#;

    #[test]
    fn test_nameless_to_toml() {
        let examples = [
            "rattler >=1",
            "conda-forge::rattler",
            "conda-forge::rattler[version=>3.0]",
            "rattler=1=*cuda",
            "rattler >=1 *cuda",
        ];

        let mut table = toml_edit::DocumentMut::new();
        for example in examples {
            let spec = MatchSpec::from_str(example, Strict)
                .unwrap()
                .into_nameless()
                .1;
            table.insert(example, Item::Value(nameless_match_spec_to_toml(&spec)));
        }
        assert_snapshot!(table);
    }

    #[test]
    fn test_get_or_insert_toml_table() {
        let mut manifest = Manifest::from_str(Path::new("pixi.toml"), PROJECT_BOILERPLATE).unwrap();
        let _ = manifest
            .document
            .get_or_insert_toml_table(None, &FeatureName::Default, "tasks")
            .map(|t| t.set_implicit(false));
        let _ = manifest
            .document
            .get_or_insert_toml_table(Some(Platform::Linux64), &FeatureName::Default, "tasks")
            .map(|t| t.set_implicit(false));
        let _ = manifest
            .document
            .get_or_insert_toml_table(None, &FeatureName::Named("test".to_string()), "tasks")
            .map(|t| t.set_implicit(false));
        let _ = manifest
            .document
            .get_or_insert_toml_table(
                Some(Platform::Linux64),
                &FeatureName::Named("test".to_string()),
                "tasks",
            )
            .map(|t| t.set_implicit(false));
        assert_snapshot!(manifest.document.to_string());
    }

    #[test]
    fn test_get_nested_toml_table_name() {
        let file_contents = r#"
[project]
name = "foo"
version = "0.1.0"
description = "foo description"
channels = []
platforms = ["linux-64", "win-64"]

        "#;

        let manifest = Manifest::from_str(Path::new("pixi.toml"), file_contents).unwrap();
        // Test all different options for the feature name and platform
        assert_eq!(
            "dependencies".to_string(),
            manifest.document.get_nested_toml_table_name(
                &FeatureName::Default,
                None,
                Some("dependencies")
            )
        );
        assert_eq!(
            "target.linux-64.dependencies".to_string(),
            manifest.document.get_nested_toml_table_name(
                &FeatureName::Default,
                Some(Platform::Linux64),
                Some("dependencies")
            )
        );
        assert_eq!(
            "feature.test.dependencies".to_string(),
            manifest.document.get_nested_toml_table_name(
                &FeatureName::Named("test".to_string()),
                None,
                Some("dependencies")
            )
        );
        assert_eq!(
            "feature.test.target.linux-64.dependencies".to_string(),
            manifest.document.get_nested_toml_table_name(
                &FeatureName::Named("test".to_string()),
                Some(Platform::Linux64),
                Some("dependencies")
            )
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
        assert_eq!(source.remove_environment("default").unwrap(), false);
        source
            .add_environment("default", Some(vec![String::from("default")]), None, false)
            .unwrap();
        assert_eq!(source.remove_environment("default").unwrap(), true);
        assert_eq!(source.remove_environment("foo").unwrap(), true);
        assert_snapshot!(
            format!("test_remove_environment_{}", source.file_name()),
            source.to_string()
        );
    }
}
