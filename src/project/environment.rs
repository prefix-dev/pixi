use crate::Project;
use rattler_shell::shell::{Shell, ShellEnum};
use std::fmt::Write;

// Add pixi meta data into the environment as environment variables.
pub fn add_metadata_as_env_vars(
    script: &mut impl Write,
    shell: &ShellEnum,
    project: &Project,
) -> anyhow::Result<()> {
    // Setting a base prefix for the pixi package
    const PREFIX: &str = "PIXI_PACKAGE_";

    shell.set_env_var(
        script,
        &format!("{PREFIX}ROOT"),
        &(project.root().to_string_lossy()),
    )?;
    shell.set_env_var(
        script,
        &format!("{PREFIX}MANIFEST"),
        &(project.manifest_path().to_string_lossy()),
    )?;
    shell.set_env_var(
        script,
        &format!("{PREFIX}PLATFORMS"),
        &(project
            .platforms()
            .iter()
            .map(|plat| plat.as_str())
            .collect::<Vec<&str>>()
            .join(",")),
    )?;
    shell.set_env_var(script, &format!("{PREFIX}NAME"), project.name())?;
    shell.set_env_var(
        script,
        &format!("{PREFIX}VERSION"),
        &project.version().to_string(),
    )?;

    Ok(())
}
