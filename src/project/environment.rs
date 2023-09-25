use crate::Project;
use itertools::Itertools;
use std::collections::HashMap;

// Setting a base prefix for the pixi package
const ENV_PREFIX: &str = "PIXI_PACKAGE_";

/// Returns environment variables and their values that should be injected when running a command.
pub fn get_metadata_env(project: &Project) -> HashMap<String, String> {
    HashMap::from_iter([
        (
            format!("{ENV_PREFIX}ROOT"),
            project.root().to_string_lossy().into_owned(),
        ),
        (format!("{ENV_PREFIX}NAME"), project.name().to_string()),
        (
            format!("{ENV_PREFIX}MANIFEST"),
            project.manifest_path().to_string_lossy().into_owned(),
        ),
        (
            format!("{ENV_PREFIX}PLATFORMS"),
            project
                .platforms()
                .iter()
                .map(|plat| plat.as_str())
                .join(","),
        ),
        (
            format!("{ENV_PREFIX}VERSION"),
            project.version().to_string(),
        ),
        ("PIXI_PROMPT".to_string(), format!("({}) ", project.name())),
    ])
}
