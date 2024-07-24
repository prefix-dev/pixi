use crate::deserialize_package_map;
use crate::Task;
use indexmap::IndexMap;
use rattler_conda_types::{NamelessMatchSpec, PackageName};
use serde::Deserialize;
use serde_with::serde_as;

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Build {
    /// The name of the build tool, for selection of build
    pub name: String,

    /// The dependencies for the build tools which will be installed in the build environment.
    /// These need to be conda packages
    #[serde(default, deserialize_with = "deserialize_package_map")]
    pub dependencies: IndexMap<PackageName, NamelessMatchSpec>,

    /// The task to run to build the project
    pub task: Task,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_build() {
        let toml = r#"
            name = "conda"
            dependencies = { "python" = ">=3.8" }
            task = { cmd = "python", inputs = ["setup.py", "build"] }
            "#;

        let build: Build = toml_edit::de::from_str(toml).unwrap();
        assert_eq!(build.name, "conda".to_string());
        assert_eq!(build.dependencies.len(), 1);
        assert_eq!(
            build.task.as_single_command().unwrap(),
            "python".to_string()
        );
        assert_eq!(
            build.task.as_execute().unwrap().inputs,
            Some(vec!["setup.py".to_string(), "build".to_string()])
        );
    }
}
