use std::{path::PathBuf, str::FromStr};

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
    project: &mut global::Project,
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

    eprintln!("installed packages: {prefix_records:?}");
    let all_executables: Vec<(String, PathBuf)> =
        prefix.find_executables(prefix_records.as_slice());

    eprintln!("all execs : {all_executables:?}");

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
                .then(|| ())
                .ok_or_else(|| miette::miette!("Binary for exposure {binary_name} is not present in the environment {env_name}"))
        })?;

    for (name_to_exposed, real_binary_to_be_exposed) in bin_names_to_expose.iter() {
        let exposed_key = ExposedKey::from_str(&name_to_exposed)?;

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
    project: &mut global::Project,
    environment_name: EnvironmentName,
    bin_names_to_remove: Vec<String>,
) -> miette::Result<()> {
    // verify that environment exist
    let exposed_by_env = project
        .environments()
        .get(&environment_name)
        .ok_or_else(|| miette::miette!("Environment {environment_name} not found"))?;

    bin_names_to_remove.iter().try_for_each(|binary_name| {
        let exposed_key = ExposedKey::from_str(binary_name)?;
        if !exposed_by_env.exposed.contains_key(&exposed_key) {
            miette::bail!("Binary {binary_name} not found in the {environment_name} environment");
        }
        Ok(())
    })?;

    let bin_env_dir = EnvDir::new(environment_name.clone()).await?;

    for binary_name in bin_names_to_remove.iter() {
        let exposed_key = ExposedKey::from_str(binary_name)?;
        // remove from map
        project
            .manifest
            .remove_exposed_binary(&environment_name, &exposed_key)?;
    }
    project.manifest.save()?;

    Ok(())
}


// mod tests {
//     use std::{env, fs};
//     use std::fs::{set_permissions, File};
//     use std::os::unix::fs::PermissionsExt;
//     use std::path::Path;
//     use std::str::FromStr;

//     use minijinja::Environment;
//     use pixi_manifest::ParsedManifest;

//     use crate::global::{self, expose_add, EnvDir, EnvironmentName, ExposedKey};
//     use crate::global::project::Manifest;


//     fn create_empty_executable_file(dir: &Path, executable_name: &str) -> miette::Result<()> {
//         // Define the path to the empty executable file
//         let file_path = dir.join("bin").join(executable_name);

//         fs::create_dir_all(file_path.parent().unwrap()).unwrap();

//         // Create the empty file
//         let _file = File::create(file_path.clone()).unwrap();

//         // Set executable permissions (Unix-like systems)
//         #[cfg(unix)]
//         {
//             let mut permissions = std::fs::metadata(file_path.clone()).unwrap().permissions();
//             // Set the executable bit (0o755 means read/write/execute for owner, read/execute for group and others)
//             permissions.set_mode(0o755);
//             set_permissions(file_path, permissions).unwrap();
//         }

//         Ok(())
//     }


//     #[tokio::test]
//     async fn test_expose_add_when_binary_exist() {
//         let contents = r#"
// [envs.python-3-10]
// channels = ["conda-forge"]
// [envs.python-3-10.dependencies]
// python = "3.10"
// [envs.python-3-10.exposed]
// python = "python"
// python3 = "python"
// "#;

//         let tmp_dir = tempfile::tempdir().unwrap();

//         let manifest_path = tmp_dir.path().join("pixi-global-manifest.toml");

//         let manifest = Manifest::from_str(&manifest_path, contents).unwrap();

//         let mut project = global::Project::from_manifest(manifest);

//         // inject a fake environment
//         let env_name = EnvironmentName::from_str("python-3-10").unwrap();

//         let env_dir = EnvDir::new(env_name.clone()).await.unwrap();

//         let test_folder_path = format!(
//             "{}/{}",
//             env!("CARGO_MANIFEST_DIR"),
//             "tests/data/conda-meta/atuin.json"
//         );

//         let content = std::fs::read_to_string(test_folder_path).unwrap();

//         let atuin_content = env_dir.path().join("conda-meta").join("atuin-18.3.0-h6e96688_0.json");
//         std::fs::create_dir_all(atuin_content.parent().unwrap()).unwrap();
//         std::fs::write(atuin_content, content).unwrap();


//         // create also an empty executable
//         create_empty_executable_file(env_dir.path(), "atuin").unwrap();


//         expose_add(&mut project, "python-3-10".parse().unwrap(), vec![("atuin".to_string(), "atuin".to_string())]).await.unwrap();

//         let exposed_key = ExposedKey::from_str("atuin").unwrap();
//         assert!(project.manifest.parsed.envs().get(&env_name).unwrap().exposed.contains_key(&exposed_key));

//         insta::assert_snapshot!(project.manifest.document.to_string());
//     }

//     #[tokio::test]
//     async fn test_expose_add_when_exposing_non_existing_binary() {
//         let contents = r#"
// [envs.python-3-10]
// channels = ["conda-forge"]
// [envs.python-3-10.dependencies]
// python = "3.10"
// [envs.python-3-10.exposed]
// python = "python"
// python3 = "python"
// "#;

//         let tmp_dir = tempfile::tempdir().unwrap();

//         let manifest_path = tmp_dir.path().join("pixi-global-manifest.toml");

//         let manifest = Manifest::from_str(&manifest_path, contents).unwrap();

//         let mut project = global::Project::from_manifest(manifest);

//         let result = expose_add(&mut project, "python-3-10".parse().unwrap(), vec![("non-existing-library".to_string(), "non-existing-library".to_string())]).await.unwrap();
//         insta::assert_snapshot!(result.unwrap_err().to_string());



//     }
// }
