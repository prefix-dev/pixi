use miette::{miette, Report};
use rattler_conda_types::Platform;
use std::fmt;
use toml_edit::{Array, Item, Table, Value};

use crate::{FeatureName, Task};

const PYPROJECT_PIXI_PREFIX: &str = "tool.pixi";

/// Discriminates between a pixi.toml and a pyproject.toml manifest
#[derive(Debug, Clone)]
pub enum ManifestSource {
    PyProjectToml(toml_edit::Document),
    PixiToml(toml_edit::Document),
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
    /// Returns the name of a nested TOML table.
    /// If `platform` and `feature_name` are `None`, the table name is returned as-is.
    /// Otherwise, the table name is prefixed with the feature, platform, or both.
    pub fn get_nested_toml_table_name(
        &self,
        feature_name: &FeatureName,
        platform: Option<Platform>,
        table: &str,
    ) -> String {
        let table_name = match (platform, feature_name) {
            (Some(platform), FeatureName::Named(_)) => format!(
                "feature.{}.target.{}.{}",
                feature_name.as_str(),
                platform.as_str(),
                table
            ),
            (Some(platform), FeatureName::Default) => {
                format!("target.{}.{}", platform.as_str(), table)
            }
            (None, FeatureName::Named(_)) => {
                format!("feature.{}.{}", feature_name.as_str(), table)
            }
            (None, FeatureName::Default) => table.to_string(),
        };

        match self {
            ManifestSource::PyProjectToml(_) => format!("{}.{}", PYPROJECT_PIXI_PREFIX, table_name),
            ManifestSource::PixiToml(_) => table_name,
        }
    }

    /// Retrieve a mutable reference to a target table `table_name`
    /// for a specific platform.
    /// If table not found, its inserted into the document.
    fn get_or_insert_toml_table<'a>(
        &'a mut self,
        platform: Option<Platform>,
        feature: &FeatureName,
        table_name: &str,
    ) -> miette::Result<&'a mut Table> {
        let table_name = self.get_nested_toml_table_name(feature, platform, table_name);
        let parts: Vec<&str> = table_name.split('.').collect();

        let mut current_table = self.as_table_mut();

        for (i, part) in parts.iter().enumerate() {
            current_table = current_table
                .entry(part)
                .or_insert(Item::Table(Table::new()))
                .as_table_mut()
                .ok_or_else(|| {
                    miette!(
                        "Could not find or access the part '{}' in the path '[{}]'",
                        part,
                        table_name
                    )
                })?;
            if i < parts.len() - 1 {
                current_table.set_dotted(true);
            }
        }
        Ok(current_table)
    }

    /// Returns a mutable reference to the specified array either in project or feature.
    pub fn specific_array_mut(
        &mut self,
        array_name: &str,
        feature_name: &FeatureName,
    ) -> miette::Result<&mut Array> {
        match feature_name {
            FeatureName::Default => {
                let project = self.get_root_table_mut("project");
                if project.is_none() {
                    *project = Item::Table(Table::new());
                }

                let channels = &mut project[array_name];
                if channels.is_none() {
                    *channels = Item::Value(Value::Array(Array::new()))
                }

                channels
                    .as_array_mut()
                    .ok_or_else(|| miette::miette!("malformed {array_name} array"))
            }
            FeatureName::Named(_) => {
                let feature = self.get_root_table_mut("feature");
                if feature.is_none() {
                    *feature = Item::Table(Table::new());
                }
                let table = feature.as_table_mut().expect("feature should be a table");
                table.set_dotted(true);

                let feature = &mut table[feature_name.as_str()];
                if feature.is_none() {
                    *feature = Item::Table(Table::new());
                }

                let channels = &mut feature[array_name];
                if channels.is_none() {
                    *channels = Item::Value(Value::Array(Array::new()))
                }

                channels
                    .as_array_mut()
                    .ok_or_else(|| miette::miette!("malformed {array_name} array"))
            }
        }
    }

    fn get_root_table_mut(&mut self, table: &str) -> &mut Item {
        match self {
            ManifestSource::PyProjectToml(document) => &mut document["tool"]["pixi"][table],
            ManifestSource::PixiToml(document) => &mut document[table],
        }
    }

    fn as_table_mut(&mut self) -> &mut Table {
        match self {
            ManifestSource::PyProjectToml(document) => document.as_table_mut(),
            ManifestSource::PixiToml(document) => document.as_table_mut(),
        }
    }

    /// Removes a conda or pypi dependency from the Toml manifest
    pub fn remove_dependency(
        &mut self,
        dep: &str,
        table: &str,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<toml_edit::Item, Report> {
        self.get_or_insert_toml_table(platform, feature_name, table)?
            .remove(dep)
            .ok_or_else(|| {
                let table_name = self.get_nested_toml_table_name(feature_name, platform, table);
                miette::miette!(
                    "Couldn't find {} in [{}]",
                    console::style(dep).bold(),
                    console::style(table_name).bold(),
                )
            })
    }

    /// Adds a conda or pypi dependency to the Toml manifest
    pub fn add_dependency(
        &mut self,
        name: &str,
        dep: Item,
        table: &str,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), Report> {
        // Find the table toml table to add the dependency to.
        let dependency_table = self.get_or_insert_toml_table(platform, feature_name, table)?;

        // Check for duplicates.
        if let Some(table_spec) = dependency_table.get(name) {
            if table_spec.to_string().trim() == dep.to_string() {
                return Err(miette::miette!(
                    "{} is already added.",
                    console::style(name).bold(),
                ));
            }
        }

        // Add the pypi dependency to the table
        dependency_table.insert(name, dep);

        Ok(())
    }

    /// Removes a task from the Toml manifest
    pub fn remove_task(
        &mut self,
        name: &str,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), Report> {
        // Get the task table either from the target platform or the default tasks.
        // If it does not exist in toml, consider this ok as we want to remove it anyways
        self.get_or_insert_toml_table(platform, feature_name, "tasks")?
            .remove(name);

        Ok(())
    }

    /// Adds a task to the Toml manifest
    pub fn add_task(
        &mut self,
        name: &str,
        task: Task,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), Report> {
        // Get the task table either from the target platform or the default tasks.
        self.get_or_insert_toml_table(platform, feature_name, "tasks")?
            .insert(name, task.into());

        Ok(())
    }
}
