use crate::Project;
use itertools::Itertools;
use rattler_shell::shell::{Shell, ShellEnum};
use std::collections::HashMap;
use std::fmt::Write;

// Setting a base prefix for the pixi package
const ENV_PREFIX: &str = "PIXI_PACKAGE_";

// Add pixi meta data into the environment as environment variables.
pub fn add_metadata_as_env_vars(
    script: &mut impl Write,
    shell: &ShellEnum,
    project: &Project,
) -> anyhow::Result<()> {
    for (key, value) in get_metadata_env(project) {
        shell.set_env_var(script, &key, &value)?;
    }

    Ok(())
}

/// Returns environment variables and their values that should be injected when running a command.
pub fn get_metadata_env(project: &Project) -> HashMap<String, String> {
    HashMap::from_iter([
        (
            format!("{ENV_PREFIX}ROOT"),
            project.root().to_string_lossy().into_owned(),
        ),
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
    ])
}
