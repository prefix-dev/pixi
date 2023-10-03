use crate::Project;
use itertools::Itertools;
use std::collections::HashMap;

// Setting a base prefix for the pixi package
const ENV_PREFIX: &str = "PIXI_PACKAGE_";

/// Returns environment variables and their values that should be injected when running a command.
pub fn get_metadata_env(project: &Project) -> HashMap<String, String> {
    #[cfg(target_os = "windows")]
    let install_prefix = project.root().join(".pixi/env/Library");
    #[cfg(not(target_os = "windows"))]
    let install_prefix = project.root().join(".pixi/env");

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
        (
            "PIXI_BUILD_FOLDER".to_string(),
            project
                .root()
                .join("pixi-build")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "PIXI_INSTALL_PREFIX".to_string(),
            install_prefix.to_string_lossy().into_owned(),
        ),
        ("PIXI_PROMPT".to_string(), format!("({}) ", project.name())),
    ])
}
