use indexmap::IndexMap;
use std::collections::HashMap;

use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::{
    activation::{
        ActivationError, ActivationError::FailedToRunActivationScript, ActivationVariables,
        Activator, PathModificationBehavior,
    },
    shell::ShellEnum,
};

use crate::project::HasProjectRef;
use crate::{project::Environment, Project};
use pixi_manifest::EnvironmentName;
use pixi_manifest::FeaturesExt;

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

impl Project {
    /// Returns environment variables and their values that should be injected when running a command.
    pub(crate) fn get_metadata_env(&self) -> HashMap<String, String> {
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
    pub(crate) fn get_metadata_env(&self) -> IndexMap<String, String> {
        let prompt = match self.name() {
            EnvironmentName::Named(name) => {
                format!("{}:{}", self.project().name(), name)
            }
            EnvironmentName::Default => self.project().name().to_string(),
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
        .extend(get_static_environment_variables(environment));

    Ok(activator)
}

/// Runs and caches the activation script.
pub async fn run_activation(
    environment: &Environment<'_>,
    env_var_behavior: &CurrentEnvVarBehavior,
) -> miette::Result<HashMap<String, String>> {
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

    Ok(activator_result)
}

/// Get the environment variables that are statically generated from the project and the environment.
/// Returns IndexMap to stay sorted, as pixi should export the metadata before exporting variables that could depend on it.
pub(crate) fn get_static_environment_variables<'p>(
    environment: &'p Environment<'p>,
) -> IndexMap<String, String> {
    // Get environment variables from the pixi project meta data
    let project_env = environment.project().get_metadata_env();

    // Add the conda default env variable so that the existing tools know about the env.
    let env_name = match environment.name() {
        EnvironmentName::Named(name) => format!("{}:{}", environment.project().name(), name),
        EnvironmentName::Default => environment.project().name().to_string(),
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
pub(crate) async fn initialize_env_variables(
    environment: &Environment<'_>,
    env_var_behavior: CurrentEnvVarBehavior,
) -> miette::Result<HashMap<String, String>> {
    let activation_env = run_activation(environment, &env_var_behavior).await?;

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
        let project = Project::from_str(Path::new("pixi.toml"), project).unwrap();
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
}
