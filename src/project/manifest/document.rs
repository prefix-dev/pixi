use miette::{miette, Report};
use rattler_conda_types::{NamelessMatchSpec, PackageName, Platform};
use std::{fmt, path::Path};
use toml_edit::{value, Array, InlineTable, Item, Table, Value};

use crate::{consts, util::default_channel_config, FeatureName, SpecType, Task};

use super::{python::PyPiPackageName, PyPiRequirement};

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
    /// Returns the name of a nested TOML table.
    /// If `platform` and `feature_name` are `None`, the table name is returned as-is.
    /// Otherwise, the table name is prefixed with the feature, platform, or both.
    fn get_nested_toml_table_name(
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

    pub fn remove_pypi_dependency(
        &mut self,
        dep: &PyPiPackageName,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), Report> {
        match self {
            ManifestSource::PixiToml(_) => self
                .remove_dependency_helper(
                    dep.as_source(),
                    consts::PYPI_DEPENDENCIES,
                    platform,
                    feature_name,
                )
                .map(|_| ()),
            ManifestSource::PyProjectToml(_) => {
                match self.as_table_mut()["project"]["dependencies"].as_array_mut() {
                    Some(array) => {
                        array.retain(|x| !x.as_str().unwrap().contains(dep.as_source()));
                        Ok(())
                    }
                    None => Ok(()), // No dependencies array, nothing to remove.
                }
            }
        }
    }

    /// Removes a conda or pypi dependency from the TOML manifest
    pub fn remove_dependency_helper(
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

    /// Adds a conda dependency to the TOML manifest
    pub fn add_dependency(
        &mut self,
        name: &PackageName,
        spec: &NamelessMatchSpec,
        spec_type: SpecType,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), Report> {
        self.add_dependency_helper(
            name.as_normalized(),
            nameless_match_spec_to_toml(spec),
            spec_type.name(),
            platform,
            feature_name,
        )
    }

    /// Add a pypi requirement to the manifest
    pub fn add_pypi_dependency(
        &mut self,
        name: &PyPiPackageName,
        requirement: &PyPiRequirement,
        platform: Option<Platform>,
        project_root: &Path,
        feature_name: &FeatureName,
    ) -> Result<(), Report> {
        match self {
            ManifestSource::PyProjectToml(_) if feature_name.is_default() => {
                let dep = requirement
                    .as_pep508(name.as_normalized(), project_root)
                    .map_err(|_| {
                        miette!("Failed to convert '{}' to pep508", &name.as_normalized())
                    })?;
                match self.as_table_mut()["project"]["dependencies"].as_array_mut() {
                    Some(array) => {
                        // Check for duplicates
                        if array
                            .iter()
                            .any(|x| x.as_str() == Some(dep.to_string().as_str()))
                        {
                            return Err(miette!(
                                "{} is already added.",
                                console::style(name.as_normalized()).bold(),
                            ));
                        }
                        array.push(dep.to_string());
                    }
                    None => {
                        self.as_table_mut()["project"]["dependencies"] =
                            Item::Value(Value::Array(Array::from_iter(vec![dep.to_string()])));
                    }
                }
                Ok(())
            }
            _ => self.add_dependency_helper(
                name.as_source(),
                requirement.clone().into(),
                consts::PYPI_DEPENDENCIES,
                platform,
                feature_name,
            ),
        }
    }

    /// Adds a conda or pypi dependency to the TOML manifest
    fn add_dependency_helper(
        &mut self,
        name: &str,
        dep: Value,
        table: &str,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), Report> {
        // Find the TOML table to add the dependency to.
        let dependency_table = self.get_or_insert_toml_table(platform, feature_name, table)?;

        // Check for duplicates
        if let Some(table_spec) = dependency_table.get(name) {
            if table_spec.as_value().and_then(|v| v.as_str()) == Some(dep.to_string().as_str()) {
                return Err(miette!("{} is already added.", console::style(name).bold(),));
            }
        }

        // Store (or replace) in the TOML document
        dependency_table.insert(name, Item::Value(dep));
        Ok(())
    }

    /// Removes a task from the TOML manifest
    pub fn remove_task(
        &mut self,
        name: &str,
        platform: Option<Platform>,
        feature_name: &FeatureName,
    ) -> Result<(), Report> {
        // Get the task table either from the target platform or the default tasks.
        // If it does not exist in TOML, consider this ok as we want to remove it anyways
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
    ) -> Result<(), Report> {
        // Get the task table either from the target platform or the default tasks.
        self.get_or_insert_toml_table(platform, feature_name, "tasks")?
            .insert(name, task.into());

        Ok(())
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

/// Given a nameless matchspec convert it into a TOML value. If the spec only contains a version a
/// string is returned, otherwise an entire table is constructed.
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
            // No other fields besides the version was specified, so we can just return the version as a string.
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
    use super::*;
    use crate::project::manifest::Manifest;
    use insta::assert_snapshot;
    use rattler_conda_types::MatchSpec;
    use rattler_conda_types::ParseStrictness::Strict;
    use std::path::Path;

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
            .get_or_insert_toml_table(None, &FeatureName::Default, "tasks");
        let _ = manifest.document.get_or_insert_toml_table(
            Some(Platform::Linux64),
            &FeatureName::Default,
            "tasks",
        );
        let _ = manifest.document.get_or_insert_toml_table(
            None,
            &FeatureName::Named("test".to_string()),
            "tasks",
        );
        let _ = manifest.document.get_or_insert_toml_table(
            Some(Platform::Linux64),
            &FeatureName::Named("test".to_string()),
            "tasks",
        );
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
                "dependencies"
            )
        );
        assert_eq!(
            "target.linux-64.dependencies".to_string(),
            manifest.document.get_nested_toml_table_name(
                &FeatureName::Default,
                Some(Platform::Linux64),
                "dependencies"
            )
        );
        assert_eq!(
            "feature.test.dependencies".to_string(),
            manifest.document.get_nested_toml_table_name(
                &FeatureName::Named("test".to_string()),
                None,
                "dependencies"
            )
        );
        assert_eq!(
            "feature.test.target.linux-64.dependencies".to_string(),
            manifest.document.get_nested_toml_table_name(
                &FeatureName::Named("test".to_string()),
                Some(Platform::Linux64),
                "dependencies"
            )
        );
    }
}
