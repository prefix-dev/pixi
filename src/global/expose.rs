use std::path::PathBuf;

use pixi_config::Config;
use rattler_shell::shell::ShellEnum;
use tokio::fs;

use crate::{
    global::{self, BinDir, EnvRoot},
    prefix::{create_activation_script, Prefix},
};

use miette::{Error, IntoDiagnostic, Report};

use super::{create_executable_scripts, script_exec_mapping, EnvDir, EnvironmentName, ExposedKey};

pub(crate) async fn expose_add(
    mut project: global::Project,
    env_name: EnvironmentName,
    bin_names_to_expose: Vec<(String, String)>,
) -> miette::Result<()> {
    // verify that environment exist
    let exposed_by_env = project
        .environments()
        .get(&env_name)
        .ok_or_else(|| miette::miette!("Environment {env_name} not found"))?;

    let bin_env_dir = EnvDir::new(env_name.clone()).await?;

    let prefix = Prefix::new(bin_env_dir.path());

    let prefix_records = prefix.find_installed_packages(None).await?;

    let all_executables: Vec<(String, PathBuf)> =
        prefix.find_executables(prefix_records.as_slice());

    let installed_binaries: Vec<&String> = all_executables
        .iter()
        .map(|(binary_name, _)| binary_name)
        .collect();

    // Check if all binaries that are to be exposed are present in the environment
    tracing::debug!("installed binaries : {installed_binaries:?}");
    tracing::debug!("binary to expose: {bin_names_to_expose:?}");

    bin_names_to_expose
        .iter()
        .try_for_each(|(_, binary_name)| {
            installed_binaries
                .contains(&binary_name)
                .then(|| {
                    println!(
                        "binary name to check {}",
                        installed_binaries.contains(&binary_name)
                    );
                    ()
                })
                .ok_or_else(|| miette::miette!("Not all binaries are present in the environment"))
        })?;

    for (name_to_exposed, real_binary_to_be_exposed) in bin_names_to_expose.iter() {
        let exposed_key: ExposedKey = name_to_exposed.parse().into_diagnostic()?;

        let script_mapping = script_exec_mapping(
            &exposed_key,
            real_binary_to_be_exposed,
            all_executables.iter(),
            &bin_env_dir.bin_dir,
            &env_name,
        )?;

        // Determine the shell to use for the invocation script
        let shell: ShellEnum = if cfg!(windows) {
            rattler_shell::shell::CmdExe.into()
        } else {
            rattler_shell::shell::Bash.into()
        };

        let activation_script = create_activation_script(&prefix, shell.clone())?;

        create_executable_scripts(&[script_mapping], &prefix, &shell, activation_script).await?;

        // Add the new binary to the manifest
        project
            .manifest
            .add_exposed_binary(
                &env_name,
                exposed_key,
                real_binary_to_be_exposed.to_string(),
            )
            .unwrap();
        project.manifest.save()?;
    }
    Ok(())
}

pub(crate) async fn expose_remove(
    mut project: global::Project,
    environment_name: EnvironmentName,
    bin_names_to_remove: Vec<String>,
) -> miette::Result<()> {
    // verify that environment exist
    let exposed_by_env = project
        .environments()
        .get(&environment_name)
        .ok_or_else(|| miette::miette!("Environment {environment_name} not found"))?;

    bin_names_to_remove.iter().try_for_each(|binary_name| {
        let exposed_key = ExposedKey::from_str(binary_name).into_diagnostic()?;
        if !exposed_by_env.exposed.contains_key(&exposed_key) {
            miette::bail!("Binary {binary_name} not found in the {environment_name} environment");
        }
        Ok(())
    })?;

    let bin_env_dir = EnvDir::new(environment_name.clone()).await?;

    for binary_name in bin_names_to_remove.iter() {
        let exposed_key = ExposedKey::from_str(binary_name).into_diagnostic()?;
        // remove from filesystem
        let bin_path = bin_env_dir.bin_dir.executable_script_path(&exposed_key);
        tracing::debug!("removing binary {bin_path:?}");
        fs::remove_file(bin_path).await.into_diagnostic()?;
        // remove from map
        project
            .manifest
            .remove_exposed_binary(&environment_name, &exposed_key)?;
    }
    project.manifest.save()?;

    Ok(())
}
