use crate::{Workspace, workspace::Environment};
use crate::{task::EnvironmentHash, workspace::HasWorkspaceRef};
use fs_err::tokio as tokio_fs;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_manifest::EnvironmentName;
use pixi_manifest::FeaturesExt;
use rattler_conda_types::Platform;
use rattler_lock::LockFile;
use rattler_shell::{
    activation::{
        ActivationError, ActivationError::FailedToRunActivationScript, ActivationVariables,
        Activator, PathModificationBehavior,
    },
    shell::ShellEnum,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// Setting a base prefix for the pixi package
const PROJECT_PREFIX: &str = "PIXI_PROJECT_";

pub enum CurrentEnvVarBehavior {
    /// Clean the environment variables of the current shell.
    /// This will return the minimal set of environment variables that are required to run the command.
    Clean,
    /// Copy the environment variables of the current shell.
    Include,
    /// Do not take any environment variables from the current shell.
    Exclude,
}

#[derive(Serialize, Deserialize)]
struct ActivationCache {
    /// The hash of the environment which produced the activation's environment variables.
    hash: EnvironmentHash,
    /// The environment variables set by the activation.
    environment_variables: HashMap<String, String>,
}

impl Workspace {
    /// Returns environment variables and their values that should be injected when running a command.
    pub(crate) fn get_metadata_env(&self) -> HashMap<String, String> {
        let mut map = HashMap::from_iter([
            (
                format!("{PROJECT_PREFIX}ROOT"),
                self.root().to_string_lossy().into_owned(),
            ),
            (
                format!("{PROJECT_PREFIX}NAME"),
                self.display_name().to_string(),
            ),
            (
                format!("{PROJECT_PREFIX}MANIFEST"),
                self.workspace
                    .provenance
                    .path
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                format!("{PROJECT_PREFIX}VERSION"),
                self.workspace
                    .value
                    .workspace
                    .version
                    .as_ref()
                    .map_or("NO_VERSION_SPECIFIED".to_string(), |version| {
                        version.to_string()
                    }),
            ),
            (String::from("PIXI_IN_SHELL"), String::from("1")),
        ]);

        if let Ok(exe_path) = std::env::current_exe() {
            map.insert(
                "PIXI_EXE".to_string(),
                exe_path.to_string_lossy().to_string(),
            );
        }

        map
    }
}

const ENV_PREFIX: &str = "PIXI_ENVIRONMENT_";

impl Environment<'_> {
    /// Returns environment variables and their values that should be injected when running a command.
    pub(crate) fn get_metadata_env(&self) -> IndexMap<String, String> {
        let prompt = match self.name() {
            EnvironmentName::Named(name) => {
                format!("{}:{}", self.workspace().display_name(), name)
            }
            EnvironmentName::Default => self.workspace().display_name().to_string(),
        };
        let mut map = IndexMap::from_iter([
            (format!("{ENV_PREFIX}NAME"), self.name().to_string()),
            (
                format!("{ENV_PREFIX}PLATFORMS"),
                self.platforms().iter().map(|plat| plat.as_str()).join(","),
            ),
            ("PIXI_PROMPT".to_string(), format!("({}) ", prompt)),
        ]);

        // Add the activation environment variables
        map.extend(self.activation_env(Some(Platform::current())));
        map
    }
}

/// Get the complete activator for the environment.
/// This method will create an activator for the environment and add the activation scripts from the project.
/// The activator will be created for the current platform and the default shell.
/// The activation scripts from the environment will be checked for existence and the extension will be checked for correctness.
pub(crate) fn get_activator<'p>(
    environment: &'p Environment<'p>,
    shell: ShellEnum,
) -> Result<Activator<ShellEnum>, ActivationError> {
    let platform = Platform::current();
    let additional_activation_scripts = environment.activation_scripts(Some(platform));

    // Make sure the scripts exists
    let (additional_activation_scripts, missing_scripts): (Vec<_>, _) =
        additional_activation_scripts
            .into_iter()
            .map(|script| environment.workspace().root().join(script))
            .partition(|full_path| full_path.is_file());

    if !missing_scripts.is_empty() {
        tracing::warn!(
            "Could not find activation scripts: {}",
            missing_scripts.iter().map(|p| p.display()).format(", ")
        );
    }

    // Check if the platform and activation script extension match. For Platform::Windows the extension should be .bat and for All other platforms it should be .sh or .bash.
    for script in additional_activation_scripts.iter() {
        let extension = script.extension().unwrap_or_default();
        if platform.is_windows() && extension != "bat" {
            tracing::warn!(
                "The activation script '{}' does not have the correct extension for the platform '{}'. The extension should be '.bat'.",
                script.display(),
                platform
            );
        } else if !platform.is_windows() && extension != "sh" && extension != "bash" {
            tracing::warn!(
                "The activation script '{}' does not have the correct extension for the platform '{}'. The extension should be '.sh' or '.bash'.",
                script.display(),
                platform
            );
        }
    }

    let mut activator =
        Activator::from_path(environment.dir().as_path(), shell, Platform::current())?;

    // Add the custom activation scripts from the environment
    activator
        .activation_scripts
        .extend(additional_activation_scripts);

    // Add the environment variables from the project.
    activator
        .env_vars
        .extend(get_static_environment_variables(environment));

    Ok(activator)
}

/// Get the environment variables from the shell environment.
/// This method retrieves the specified environment variables from the shell and returns them as a HashMap.
/// If the variable is not set, its value will be `None`.
fn get_environment_variable_from_shell_environment(
    names: Vec<&str>,
) -> HashMap<String, Option<String>> {
    names
        .into_iter()
        .map(|name| {
            let value = std::env::var(name).ok();
            (name.to_string(), value)
        })
        .collect()
}

/// Try to get the activation cache from the cache file.
/// If it can get the cache, it will validate it with the lock file and the current environment.
/// If the cache is valid, it will return the environment variables from the cache.
///
/// Without a lock file it will not use the cache, as it indicates the cache is not interesting
async fn try_get_valid_activation_cache(
    lock_file: &LockFile,
    environment: &Environment<'_>,
    cache_file: PathBuf,
) -> Option<HashMap<String, String>> {
    // Find cache file
    if !cache_file.exists() {
        return None;
    }
    // Read the cache file
    let cache_content = match tokio_fs::read_to_string(&cache_file).await {
        Ok(content) => content,
        Err(e) => {
            tracing::debug!("Failed to read activation cache file, reactivating. Error: {e}");
            return None;
        }
    };
    // Parse the cache file
    let cache: ActivationCache = match serde_json::from_str(&cache_content) {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::debug!("Failed to parse cache file, reactivating. Error: {e}");
            return None;
        }
    };

    // Get the current environment variables
    let current_input_env_vars = get_environment_variable_from_shell_environment(
        cache
            .environment_variables
            .keys()
            .map(String::as_str)
            .collect(),
    );

    // Hash the current state
    let hash = EnvironmentHash::from_environment(environment, &current_input_env_vars, lock_file);

    // Check if the hash matches
    if cache.hash == hash {
        Some(cache.environment_variables)
    } else {
        None
    }
}

/// Runs and caches the activation script.
pub async fn run_activation(
    environment: &Environment<'_>,
    env_var_behavior: &CurrentEnvVarBehavior,
    lock_file: Option<&LockFile>,
    force_activate: bool,
    experimental: bool,
) -> miette::Result<HashMap<String, String>> {
    // If the user requested to use the cache and the lockfile is provided, we can try to use the cache.
    if !force_activate && experimental {
        let cache_file = environment
            .workspace()
            .activation_env_cache_folder()
            .join(environment.activation_cache_name());
        if let Some(lock_file) = lock_file {
            if let Some(env_vars) =
                try_get_valid_activation_cache(lock_file, environment, cache_file).await
            {
                tracing::debug!("Using activation cache for {:?}", environment.name());
                return Ok(env_vars);
            }
        } else {
            tracing::debug!(
                "No lock file provided for activation, not using activation cache for {:?}",
                environment.name()
            );
        }
    }
    tracing::debug!("Running activation script for {:?}", environment.name());

    let activator = get_activator(environment, ShellEnum::default()).map_err(|e| {
        miette::miette!(format!(
            "failed to create activator for {:?}\n{}",
            environment.name(),
            e
        ))
    })?;

    let path_modification_behavior = match env_var_behavior {
        // We need to replace the full environment path with the new one.
        // So only the executables from the pixi environment are available.
        CurrentEnvVarBehavior::Clean => PathModificationBehavior::Replace,
        _ => PathModificationBehavior::Prepend,
    };

    let activator_result = match tokio::task::spawn_blocking(move || {
        // Run and cache the activation script
        activator.run_activation(
            ActivationVariables {
                // Get the current PATH variable
                path: Default::default(),

                // Start from an empty prefix
                conda_prefix: None,

                // Prepending environment paths so they get found first.
                path_modification_behavior,

                // Current environment variables
                current_env: HashMap::new(),
            },
            None,
        )
    })
    .await
    .into_diagnostic()?
    {
        Ok(activator) => activator,
        Err(e) => {
            match e {
                FailedToRunActivationScript {
                    script,
                    stdout,
                    stderr,
                    status,
                } => {
                    return Err(miette::miette!(format!(
                        "Failed to run activation script for {:?}. Status: {}. Stdout: {}. Stderr: {}. Script: {}",
                        environment.name(), // Make sure `environment` is accessible here
                        status,
                        stdout,
                        stderr,
                        script,
                    )));
                }
                _ => {
                    // Handle other activation errors
                    return Err(miette::miette!(format!(
                        "An activation error occurred: {:?}",
                        e
                    )));
                }
            }
        }
    };

    // If the lock file is provided, and we can compute the environment hash, let's rewrite the
    // cache file.
    if experimental {
        if let Some(lock_file) = lock_file {
            // Get the current environment variables from the shell to be part of the hash
            let current_input_env_vars = get_environment_variable_from_shell_environment(
                activator_result.keys().map(String::as_str).collect(),
            );
            let cache_file = environment.activation_cache_file_path();
            let cache = ActivationCache {
                hash: EnvironmentHash::from_environment(
                    environment,
                    &current_input_env_vars,
                    lock_file,
                ),
                environment_variables: activator_result.clone(),
            };
            let cache = serde_json::to_string(&cache).into_diagnostic()?;

            tokio_fs::create_dir_all(environment.workspace().activation_env_cache_folder())
                .await
                .into_diagnostic()?;
            tokio_fs::write(&cache_file, cache)
                .await
                .into_diagnostic()?;
            tracing::debug!(
                "Wrote activation cache for {} to {}",
                environment.name(),
                cache_file.display()
            );
        }
    }

    Ok(activator_result)
}

/// Get the environment variables that are statically generated from the project and the environment.
/// Returns IndexMap to stay sorted, as pixi should export the metadata before exporting variables that could depend on it.
pub(crate) fn get_static_environment_variables<'p>(
    environment: &'p Environment<'p>,
) -> IndexMap<String, String> {
    // Get environment variables from the pixi project meta data
    let project_env = environment.workspace().get_metadata_env();

    // Add the conda default env variable so that the existing tools know about the env.
    let env_name = match environment.name() {
        EnvironmentName::Named(name) => {
            format!("{}:{}", environment.workspace().display_name(), name)
        }
        EnvironmentName::Default => environment.workspace().display_name().to_string(),
    };
    let mut shell_env = HashMap::new();
    shell_env.insert("CONDA_DEFAULT_ENV".to_string(), env_name);

    // Get environment variables from the pixi environment
    let environment_env = environment.get_metadata_env();

    // Combine the environments
    project_env
        .into_iter()
        .chain(shell_env)
        .chain(environment_env)
        .collect()
}

/// Get the environment variables that are set in the current shell
/// and strip them down to the minimal set required to run a command.
pub(crate) fn get_clean_environment_variables() -> HashMap<String, String> {
    let env = std::env::vars().collect::<HashMap<_, _>>();

    let unix_keys = if cfg!(unix) {
        vec![
            "DISPLAY",
            "LC_ALL",
            "LC_TIME",
            "LC_NUMERIC",
            "LC_MEASUREMENT",
            "SHELL",
            "USER",
            "USERNAME",
            "LOGNAME",
            "HOME",
            "HOSTNAME",
        ]
    } else {
        vec![]
    };

    let macos_keys = if cfg!(target_os = "macos") {
        vec!["TMPDIR", "XPC_SERVICE_NAME", "XPC_FLAGS"]
    } else {
        vec![]
    };

    let keys = unix_keys
        .into_iter()
        .chain(macos_keys)
        // .chain(windows_keys)
        .map(|s| s.to_string().to_uppercase())
        .collect_vec();

    env.into_iter()
        .filter(|(key, _)| keys.contains(&key.to_string().to_uppercase()))
        .collect::<HashMap<String, String>>()
}

/// Determine the environment variables that need to be set in an interactive shell to make it
/// function as if the environment has been activated. This method runs the activation scripts from
/// the environment and stores the environment variables it added, finally it adds environment
/// variables from the project and based on the clean_env setting it will also add in the current
/// shell environment variables.
///
/// If a lock file is given this will also create/use an activated environment cache when possible.
pub(crate) async fn initialize_env_variables(
    environment: &Environment<'_>,
    env_var_behavior: CurrentEnvVarBehavior,
    lock_file: Option<&LockFile>,
    force_activate: bool,
    experimental: bool,
) -> miette::Result<HashMap<String, String>> {
    let activation_env = run_activation(
        environment,
        &env_var_behavior,
        lock_file,
        force_activate,
        experimental,
    )
    .await?;

    // Get environment variables from the currently activated shell.
    let current_shell_env_vars = match env_var_behavior {
        CurrentEnvVarBehavior::Clean if cfg!(windows) => {
            return Err(miette::miette!(
                "Currently it's not possible to run a `clean-env` option on Windows."
            ));
        }
        CurrentEnvVarBehavior::Clean => get_clean_environment_variables(),
        CurrentEnvVarBehavior::Include => std::env::vars().collect(),
        CurrentEnvVarBehavior::Exclude => HashMap::new(),
    };

    let all_variables: HashMap<String, String> = current_shell_env_vars
        .into_iter()
        .chain(activation_env)
        .collect();

    Ok(all_variables)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::str::FromStr;

    #[test]
    fn test_metadata_env() {
        let multi_env_project = r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64", "osx-64", "win-64"]

        [activation.env]
        TEST = "123test123"

        [feature.test.dependencies]
        pytest = "*"
        [environments]
        test = ["test"]
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), multi_env_project).unwrap();

        let default_env = project.default_environment();
        let env = default_env.get_metadata_env();

        assert_eq!(env.get("PIXI_ENVIRONMENT_NAME").unwrap(), "default");
        assert!(env.get("PIXI_ENVIRONMENT_PLATFORMS").is_some());
        assert!(env.get("PIXI_PROMPT").unwrap().contains("pixi"));

        let test_env = project.environment("test").unwrap();
        let env = test_env.get_metadata_env();

        assert_eq!(env.get("PIXI_ENVIRONMENT_NAME").unwrap(), "test");
        assert!(env.get("PIXI_PROMPT").unwrap().contains("pixi"));
        assert!(env.get("PIXI_PROMPT").unwrap().contains("test"));
        assert!(env.get("TEST").unwrap().contains("123test123"));
    }

    #[test]
    fn test_metadata_project_env() {
        let project = r#"
        [project]
        name = "pixi"
        version = "0.1.0"
        channels = ["conda-forge"]
        platforms = ["linux-64", "osx-64", "win-64"]
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), project).unwrap();
        let env = project.get_metadata_env();

        assert_eq!(
            env.get("PIXI_PROJECT_NAME").unwrap(),
            project.display_name()
        );
        assert_eq!(
            env.get("PIXI_PROJECT_ROOT").unwrap(),
            project.root().to_str().unwrap()
        );
        assert_eq!(
            env.get("PIXI_PROJECT_MANIFEST").unwrap(),
            project.workspace.provenance.path.to_str().unwrap()
        );
        assert_eq!(
            env.get("PIXI_PROJECT_VERSION").unwrap(),
            &project
                .workspace
                .value
                .workspace
                .version
                .as_ref()
                .unwrap()
                .to_string()
        );
    }

    #[test]
    fn test_metadata_project_env_order() {
        let project = r#"
        [project]
        name = "pixi"
        channels = [""]
        platforms = ["linux-64", "osx-64", "win-64"]

        [activation.env]
        ABC = "123test123"
        ZZZ = "123test123"
        ZAB = "123test123"
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), project).unwrap();
        let env = get_static_environment_variables(&project.default_environment());

        // Make sure the user defined environment variables are at the end.
        assert!(
            env.keys().position(|key| key == "PIXI_PROJECT_NAME")
                < env.keys().position(|key| key == "ABC")
        );
        assert!(
            env.keys().position(|key| key == "PIXI_PROJECT_NAME")
                < env.keys().position(|key| key == "ZZZ")
        );

        // Make sure the user defined environment variables are sorted by input order.
        assert!(env.keys().position(|key| key == "ABC") < env.keys().position(|key| key == "ZZZ"));
        assert!(env.keys().position(|key| key == "ZZZ") < env.keys().position(|key| key == "ZAB"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_get_linux_clean_environment_variables() {
        let env = get_clean_environment_variables();
        // Make sure that the environment variables are set.
        assert_eq!(
            env.get("USER").unwrap(),
            std::env::var("USER").as_ref().unwrap()
        );
    }

    /// Test that the activation cache is created and used correctly based on the lockfile.
    ///
    /// This test will validate the cache usages by running the activation script and checking if the cache is created.
    /// - It will then modify the cache and check if the cache is used.
    /// - It will then modify the lock file and check if the cache is not used and recreated.
    /// - It will then modify the cache again and check if the cache is used again.
    #[tokio::test]
    async fn test_run_activation_cache_based_on_lockfile() {
        let temp_dir = tempfile::tempdir().unwrap();
        let project = r#"
        [project]
        name = "pixi"
        channels = []
        platforms = []

        [activation.env]
        TEST = "ACTIVATION123"
        "#;
        let project =
            Workspace::from_str(temp_dir.path().join("pixi.toml").as_path(), project).unwrap();
        let default_env = project.default_environment();

        // Don't create cache, by not giving it a lockfile
        let env = run_activation(
            &default_env,
            &CurrentEnvVarBehavior::Include,
            None,
            false,
            true,
        )
        .await
        .unwrap();
        assert!(!project.activation_env_cache_folder().exists());
        assert!(env.contains_key("CONDA_PREFIX"));

        // Create cache
        let lock_file = LockFile::default();
        let _env = run_activation(
            &default_env,
            &CurrentEnvVarBehavior::Include,
            Some(&lock_file),
            false,
            true,
        )
        .await
        .unwrap();
        assert!(project.activation_env_cache_folder().exists());
        assert!(
            project
                .activation_env_cache_folder()
                .join(project.default_environment().activation_cache_name())
                .exists()
        );

        // Verify that the cache is used, by overwriting the cache and checking if that persisted
        let cache_file = project.default_environment().activation_cache_file_path();
        let contents = tokio_fs::read_to_string(&cache_file).await.unwrap();
        let modified = contents.replace("ACTIVATION123", "ACTIVATION456");
        tokio_fs::write(&cache_file, modified).await.unwrap();

        let env = run_activation(
            &default_env,
            &CurrentEnvVarBehavior::Include,
            Some(&lock_file),
            false,
            true,
        )
        .await
        .unwrap();
        assert_eq!(env.get("TEST").unwrap(), "ACTIVATION456");

        // Verify that the cache is not used when the hash is different.
        //
        // We change the hash by modifying the lock-file. This should invalidate the cache and thus
        // result in the activation script being run again.
        let mock_lock = &format!(
            r#"
version: 6
environments:
  default:
    channels:
    - url: https://prefix.dev/conda-forge/
    packages:
      {platform}:
      - conda: https://prefix.dev/conda-forge/noarch/_r-mutex-1.0.1-anacondar_1.tar.bz2
packages:
- conda: https://prefix.dev/conda-forge/noarch/_r-mutex-1.0.1-anacondar_1.tar.bz2
  sha256: e58f9eeb416b92b550e824bcb1b9fb1958dee69abfe3089dfd1a9173e3a0528a
  md5: 19f9db5f4f1b7f5ef5f6d67207f25f38
  license: BSD
  size: 3566
  timestamp: 1562343890778
"#,
            platform = Platform::current()
        );
        let lock_file = LockFile::from_str(mock_lock).unwrap();
        let env = run_activation(
            &default_env,
            &CurrentEnvVarBehavior::Include,
            Some(&lock_file),
            false,
            true,
        )
        .await
        .unwrap();
        assert_eq!(env.get("TEST").unwrap(), "ACTIVATION123");

        // Verify that the cache is used again after the hash is the same
        let contents = tokio_fs::read_to_string(&cache_file).await.unwrap();
        let modified = contents.replace("ACTIVATION123", "ACTIVATION456");
        tokio_fs::write(&cache_file, modified).await.unwrap();

        let env = run_activation(
            &default_env,
            &CurrentEnvVarBehavior::Include,
            Some(&lock_file),
            false,
            true,
        );
        assert_eq!(env.await.unwrap().get("TEST").unwrap(), "ACTIVATION456");
    }

    #[tokio::test]
    async fn test_run_activation_cache_based_on_activation_env() {
        let temp_dir = tempfile::tempdir().unwrap();
        let project = r#"
        [project]
        name = "pixi"
        channels = []
        platforms = []

        [activation.env]
        TEST = "ACTIVATION123"
        "#;
        let project =
            Workspace::from_str(temp_dir.path().join("pixi.toml").as_path(), project).unwrap();
        let default_env = project.default_environment();
        let env = run_activation(
            &default_env,
            &CurrentEnvVarBehavior::Include,
            Some(&LockFile::default()),
            false,
            true,
        )
        .await
        .unwrap();
        assert_eq!(env.get("TEST").unwrap(), "ACTIVATION123",);

        // Modify the variable in cache
        let cache_file = project.default_environment().activation_cache_file_path();
        let contents = tokio_fs::read_to_string(&cache_file).await.unwrap();
        let modified = contents.replace("ACTIVATION123", "ACTIVATION456");
        tokio_fs::write(&cache_file, modified).await.unwrap();

        // Check that the cache is invalidated when the activation.env changes.
        let project = r#"
        [project]
        name = "pixi"
        channels = []
        platforms = []

        [activation.env]
        TEST = "ACTIVATION123"
        TEST2 = "ACTIVATION1234"
        "#;
        let project =
            Workspace::from_str(temp_dir.path().join("pixi.toml").as_path(), project).unwrap();
        let default_env = project.default_environment();
        let env = run_activation(
            &default_env,
            &CurrentEnvVarBehavior::Include,
            Some(&LockFile::default()),
            false,
            true,
        )
        .await
        .unwrap();
        assert_eq!(
            env.get("TEST").unwrap(),
            "ACTIVATION123",
            "The old variable should be reset"
        );
        assert_eq!(
            env.get("TEST2").unwrap(),
            "ACTIVATION1234",
            "The new variable should be set"
        );
    }

    // This test works, most of the times.., so this is a good test to run locally.
    // But it is to flaky for CI unfortunately!
    // #[tokio::test]
    // async fn test_run_activation_based_on_existing_env(){
    //     let temp_dir = tempfile::tempdir().unwrap();
    //     let project = r#"
    //     [project]
    //     name = "pixi"
    //     channels = []
    //     platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]
    //
    //     [target.unix.activation.env]
    //     TEST_ENV_VAR = "${TEST_ENV_VAR}_and_some_more"
    //
    //     [target.win.activation.env]
    //     TEST_ENV_VAR = "%TEST_ENV_VAR%_and_some_more"
    //     "#;
    //     let project =
    //         Project::from_str(temp_dir.path().join("pixi.toml").as_path(), project).unwrap();
    //     let default_env = project.default_environment();
    //
    //     // Set the environment variable
    //     std::env::set_var("TEST_ENV_VAR", "test_value");
    //
    //     // Run the activation script
    //     let env = run_activation(
    //         &default_env,
    //         &CurrentEnvVarBehavior::Include,
    //         Some(&LockFile::default()),
    //         false,
    //         true,
    //     ).await.unwrap();
    //
    //     // Check that the environment variable is set correctly
    //     assert_eq!(env.get("TEST_ENV_VAR").unwrap(), "test_value_and_some_more");
    //
    //     // Modify the environment variable
    //     let cache_file = project.default_environment().activation_cache_file_path();
    //     let contents = tokio_fs::read_to_string(&cache_file).await.unwrap();
    //     let modified = contents.replace("test_value_and_some_more", "modified_cache");
    //     tokio_fs::write(&cache_file, modified).await.unwrap();
    //
    //     // Run the activation script
    //     let env = run_activation(
    //         &default_env,
    //         &CurrentEnvVarBehavior::Include,
    //         Some(&LockFile::default()),
    //         false,
    //         true,
    //     ).await.unwrap();
    //
    //     // Check that the environment variable is taken from cache
    //     assert_eq!(env.get("TEST_ENV_VAR").unwrap(), "modified_cache");
    //
    //     // Reset the environment variable
    //     std::env::set_var("TEST_ENV_VAR", "different_test_value");
    //
    //     // Run the activation script
    //     let env = run_activation(
    //         &default_env,
    //         &CurrentEnvVarBehavior::Include,
    //         Some(&LockFile::default()),
    //         false,
    //         true,
    //     ).await.unwrap();
    //
    //     // Check that the environment variable reset, thus the cache was invalidated.
    //     assert_eq!(env.get("TEST_ENV_VAR").unwrap(), "different_test_value_and_some_more");
    //
    //     // Unset the environment variable
    //     std::env::remove_var("TEST_ENV_VAR");
    // }
}
