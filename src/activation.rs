use std::collections::HashMap;

use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::activation::ActivationError::FailedToRunActivationScript;
use rattler_shell::{
    activation::{ActivationError, ActivationVariables, Activator, PathModificationBehavior},
    shell::ShellEnum,
};

use crate::project::has_features::HasFeatures;
use crate::{
    environment::{get_up_to_date_prefix, LockFileUsage},
    project::{manifest::EnvironmentName, Environment},
    Project,
};

// Setting a base prefix for the pixi package
const PROJECT_PREFIX: &str = "PIXI_PROJECT_";

impl Project {
    /// Returns environment variables and their values that should be injected when running a command.
    pub fn get_metadata_env(&self) -> HashMap<String, String> {
        let mut map = HashMap::from_iter([
            (
                format!("{PROJECT_PREFIX}ROOT"),
                self.root().to_string_lossy().into_owned(),
            ),
            (format!("{PROJECT_PREFIX}NAME"), self.name().to_string()),
            (
                format!("{PROJECT_PREFIX}MANIFEST"),
                self.manifest_path().to_string_lossy().into_owned(),
            ),
            (
                format!("{PROJECT_PREFIX}VERSION"),
                self.version()
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
    pub fn get_metadata_env(&self) -> HashMap<String, String> {
        let prompt = match self.name() {
            EnvironmentName::Named(name) => {
                format!("{}:{}", self.project().name(), name)
            }
            EnvironmentName::Default => self.project().name().to_string(),
        };
        let mut map = HashMap::from_iter([
            (format!("{ENV_PREFIX}NAME"), self.name().to_string()),
            (
                format!("{ENV_PREFIX}PLATFORMS"),
                self.platforms().iter().map(|plat| plat.as_str()).join(","),
            ),
            ("PIXI_PROMPT".to_string(), format!("({}) ", prompt)),
        ]);
        map.extend(self.activation_env(Some(Platform::current())));
        map
    }
}

/// Get the complete activator for the environment.
/// This method will create an activator for the environment and add the activation scripts from the project.
/// The activator will be created for the current platform and the default shell.
/// The activation scripts from the environment will be checked for existence and the extension will be checked for correctness.
pub fn get_activator<'p>(
    environment: &'p Environment<'p>,
    shell: ShellEnum,
) -> Result<Activator<ShellEnum>, ActivationError> {
    let platform = Platform::current();
    let additional_activation_scripts = environment.activation_scripts(Some(platform));

    // Make sure the scripts exists
    let (additional_activation_scripts, missing_scripts): (Vec<_>, _) =
        additional_activation_scripts
            .into_iter()
            .map(|script| environment.project().root().join(script))
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
            tracing::warn!("The activation script '{}' does not have the correct extension for the platform '{}'. The extension should be '.bat'.", script.display(), platform);
        } else if !platform.is_windows() && extension != "sh" && extension != "bash" {
            tracing::warn!("The activation script '{}' does not have the correct extension for the platform '{}'. The extension should be '.sh' or '.bash'.", script.display(), platform);
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
        .extend(get_environment_variables(environment));

    Ok(activator)
}

/// Runs and caches the activation script.
pub async fn run_activation(
    environment: &Environment<'_>,
    clean_env: bool,
) -> miette::Result<HashMap<String, String>> {
    let activator = get_activator(environment, ShellEnum::default()).map_err(|e| {
        miette::miette!(format!(
            "failed to create activator for {:?}\n{}",
            environment.name(),
            e
        ))
    })?;

    let path_modification_behavior = if clean_env {
        PathModificationBehavior::Replace
    } else {
        PathModificationBehavior::Prepend
    };

    let activator_result = match tokio::task::spawn_blocking(move || {
        // Run and cache the activation script
        activator.run_activation(ActivationVariables {
            // Get the current PATH variable
            path: Default::default(),

            // Start from an empty prefix
            conda_prefix: None,

            // Prepending environment paths so they get found first.
            path_modification_behavior,
        })
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

    if clean_env && cfg!(windows){
        return Err(miette::miette!(format!("It's not possible to run a `clean-env` on windows as it will create to much issues for the running programs which makes it practically useless")))
    }
    else if clean_env {
        let mut cleaned_environment_variables = get_clean_environment_variables();

        // Extend with the original activation environment
        cleaned_environment_variables.extend(activator_result);

        // Enable this when we found a better way to support windows.
        // On Windows the path is not completly replace but we need to strip some paths to keep it as clean as possible.
        // if cfg!(target_os = "windows") {
        //     let path = env
        //         .get("Path")
        //         .map(|path| {
        //             // Keep some of the paths
        //             let win_path = std::env::split_paths(&path).filter(|p| {
        //                 // Required for base functionalities
        //                 p.to_string_lossy().contains(":\\Windows")
        //                     // Required for compilers
        //                     || p.to_string_lossy().contains("\\Program Files")
        //                     // Required for pixi environments
        //                     || p.starts_with(environment.dir())
        //             });
        //             // Join back up the paths
        //             std::env::join_paths(win_path).expect("Could not join paths")
        //         })
        //         .expect("Could not find PATH in environment variables");
        //     // Insert the path back into the env.
        //     env.insert(
        //         "Path".to_string(),
        //         path.to_str()
        //             .expect("Path contains non-utf8 paths")
        //             .to_string(),
        //     );
        // }

        return Ok(cleaned_environment_variables);
    }
    Ok(std::env::vars().chain(activator_result))
}

/// Get the environment variables that are statically generated from the project and the environment.
pub fn get_environment_variables<'p>(environment: &'p Environment<'p>) -> HashMap<String, String> {
    // Get environment variables from the project
    let project_env = environment.project().get_metadata_env();

    // Get environment variables from the environment
    let environment_env = environment.get_metadata_env();

    // Add the conda default env variable so that the existing tools know about the env.
    let env_name = match environment.name() {
        EnvironmentName::Named(name) => format!("{}:{}", environment.project().name(), name),
        EnvironmentName::Default => environment.project().name().to_string(),
    };
    let mut shell_env = HashMap::new();
    shell_env.insert("CONDA_DEFAULT_ENV".to_string(), env_name);

    // Combine the environments
    project_env
        .into_iter()
        .chain(environment_env)
        .chain(shell_env)
        .collect()
}

pub fn get_clean_environment_variables() -> HashMap<String, String> {
    let env = std::env::vars().collect::<HashMap<_, _>>();

    let unix_keys = if cfg!(target_os = "unix") {
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
/// variables from the project.
pub async fn get_activation_env<'p>(
    environment: &'p Environment<'p>,
    lock_file_usage: LockFileUsage,
) -> miette::Result<&HashMap<String, String>> {
    // Get the prefix which we can then activate.
    get_up_to_date_prefix(environment, lock_file_usage, false).await?;

    environment
        .project()
        .get_env_variables(environment, false)
        .await
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

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
        let project = Project::from_str(Path::new("pixi.toml"), multi_env_project).unwrap();

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
        let project = Project::from_str(Path::new("pixi.toml"), project).unwrap();
        let env = project.get_metadata_env();

        assert_eq!(env.get("PIXI_PROJECT_NAME").unwrap(), project.name());
        assert_eq!(
            env.get("PIXI_PROJECT_ROOT").unwrap(),
            project.root().to_str().unwrap()
        );
        assert_eq!(
            env.get("PIXI_PROJECT_MANIFEST").unwrap(),
            project.manifest_path().to_str().unwrap()
        );
        assert_eq!(
            env.get("PIXI_PROJECT_VERSION").unwrap(),
            &project.version().as_ref().unwrap().to_string()
        );
    }

    #[test]
    #[cfg(target_os = "unix")]
    fn test_get_linux_clean_environment_variables() {
        let env = get_clean_environment_variables();
        // Make sure that the environment variables are set.
        assert_eq!(
            env.get("USER").unwrap(),
            std::env::var("USER").as_ref().unwrap()
        );
    }
}
