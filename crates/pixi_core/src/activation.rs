use crate::{Workspace, workspace::Environment};
use crate::{environment::EnvironmentHash, workspace::HasWorkspaceRef};
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
    /// Hash of the environment that produced these activation results.
    /// Captures input env vars, activation scripts, project activation
    /// env vars, and either:
    /// - the prefix's install fingerprint (preferred — see
    ///   [`InstallPixiEnvironmentResult::installed_fingerprint`]), or
    /// - locked package URLs (legacy fallback when no fingerprint is
    ///   stored next to the prefix).
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

        IndexMap::from_iter([
            (format!("{ENV_PREFIX}NAME"), self.name().to_string()),
            (
                format!("{ENV_PREFIX}PLATFORMS"),
                self.platforms().iter().map(|plat| plat.as_str()).join(","),
            ),
            ("PIXI_PROMPT".to_string(), format!("({prompt}) ")),
        ])
    }
}

/// Get the complete activator for the environment.
/// This method will create an activator for the environment and add the activation scripts from the project.
/// The activator will be created for the current platform and the default shell.
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
            .map(|script| environment.workspace().root().join(script))
            .partition(|full_path| full_path.is_file());

    if !missing_scripts.is_empty() {
        tracing::warn!(
            "Could not find activation scripts: {}",
            missing_scripts
                .iter()
                .map(|p| p.display())
                .format(", ")
                .to_string()
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

    // Add the environment variables from the project (pre-activation script vars).
    activator
        .env_vars
        .extend(get_static_environment_variables(environment));

    // Add environment variables that should be applied after activation scripts run.
    activator
        .post_activation_env_vars
        .extend(environment.activation_env(Some(Platform::current())));

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
/// Try to load a still-valid activation cache for `environment`.
///
/// Cache hits require:
/// 1. The cache file exists and parses.
/// 2. An install fingerprint marker exists for the environment (set
///    by `LockFileDerivedData::prefix` after a successful install).
/// 3. The cache's recorded hash matches a freshly-computed
///    [`EnvironmentHash::for_activation`] using that fingerprint.
///
/// Returns `None` on any other outcome — including a missing
/// fingerprint marker — so the caller falls through to running
/// activation fresh.
async fn try_get_valid_activation_cache(
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

    // The activation cache is keyed on the prefix's install
    // fingerprint (see `EnvironmentHash::for_activation`). Without one
    // we have no authoritative summary of what's in the prefix and
    // can't reuse a cached activation safely, so treat the cache as
    // missing. The fingerprint is written by
    // `LockFileDerivedData::prefix` after a successful install.
    let installed_fingerprint =
        pixi_command_dispatcher::EnvironmentFingerprint::read(&environment.dir())?;
    let hash = EnvironmentHash::for_activation(
        environment,
        &current_input_env_vars,
        &installed_fingerprint,
    );

    // Check if the hash matches
    if cache.hash == hash {
        Some(cache.environment_variables)
    } else {
        None
    }
}

/// Runs and caches the activation script.
///
/// The `_lock_file` parameter is retained for API stability (several
/// callers still pass one) but is no longer consulted: the activation
/// cache is now keyed on the prefix's install fingerprint instead of
/// locked package URLs (see [`EnvironmentHash::for_activation`]).
#[allow(clippy::needless_pass_by_value)]
pub async fn run_activation(
    environment: &Environment<'_>,
    env_var_behavior: &CurrentEnvVarBehavior,
    _lock_file: Option<&LockFile>,
    force_activate: bool,
    experimental: bool,
) -> miette::Result<HashMap<String, String>> {
    // Try the activation cache first. The cache is keyed on the
    // prefix's install fingerprint
    // (`InstallPixiEnvironmentResult::installed_fingerprint`), which
    // changes whenever any package's content changes — so a cache
    // hit means the activation env-var map is still authoritative.
    // The inner lookup short-circuits to `None` when no fingerprint
    // marker exists yet (e.g. before the first install), so this
    // path is always safe to attempt.
    //
    if !force_activate && experimental {
        let cache_file = environment
            .workspace()
            .activation_env_cache_folder()
            .join(environment.activation_cache_name());
        if let Some(env_vars) = try_get_valid_activation_cache(environment, cache_file).await {
            tracing::debug!("Using activation cache for {:?}", environment.name());
            return Ok(env_vars);
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
        // Current environment variables
        let current_env = std::env::vars().collect::<HashMap<_, _>>();

        // Run and cache the activation script
        activator.run_activation(
            ActivationVariables {
                // Get the current PATH variable
                path: Default::default(),

                // Start from an empty prefix
                conda_prefix: None,

                // Prepending environment paths so they get found first.
                path_modification_behavior,

                // The current environment variables from the shell
                current_env,
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

    // Persist the cache so future calls can short-circuit. Same
    // gate as the read side — when caching is disabled or no
    // install fingerprint is available we just return the
    // freshly-computed activation result.
    if experimental {
        // Get the current environment variables from the shell to be part of the hash
        let current_input_env_vars = get_environment_variable_from_shell_environment(
            activator_result.keys().map(String::as_str).collect(),
        );
        let cache_file = environment.activation_cache_file_path();
        // Skip the write side too when no install fingerprint is
        // available — the read side won't be able to produce a
        // matching key, so any cache file we wrote would never be
        // reused. Falling through here just means the next
        // activation will re-run; correctness over a tempting but
        // dead cache.
        let Some(installed_fingerprint) =
            pixi_command_dispatcher::EnvironmentFingerprint::read(&environment.dir())
        else {
            return Ok(activator_result);
        };
        let cache = ActivationCache {
            hash: EnvironmentHash::for_activation(
                environment,
                &current_input_env_vars,
                &installed_fingerprint,
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

    #[test]
    fn test_metadata_env() {
        let multi_env_workspace = r#"
        [workspace]
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
        let project = Workspace::from_str(Path::new("pixi.toml"), multi_env_workspace).unwrap();

        let default_env = project.default_environment();
        let env = default_env.get_metadata_env();

        assert_eq!(env.get("PIXI_ENVIRONMENT_NAME").unwrap(), "default");
        assert!(env.get("PIXI_ENVIRONMENT_PLATFORMS").is_some());
        assert!(env.get("PIXI_PROMPT").unwrap().contains("pixi"));

        let test_env = project.environment("test").unwrap();
        let env = test_env.get_metadata_env();
        let post_activation_env = test_env.activation_env(Some(Platform::current()));

        assert_eq!(env.get("PIXI_ENVIRONMENT_NAME").unwrap(), "test");
        assert!(env.get("PIXI_PROMPT").unwrap().contains("pixi"));
        assert!(env.get("PIXI_PROMPT").unwrap().contains("test"));
        assert!(
            post_activation_env
                .get("TEST")
                .unwrap()
                .contains("123test123")
        );
    }

    #[test]
    fn test_metadata_project_env() {
        let workspace = r#"
        [workspace]
        name = "pixi"
        version = "0.1.0"
        channels = ["conda-forge"]
        platforms = ["linux-64", "osx-64", "win-64"]
        "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), workspace).unwrap();
        let env = workspace.get_metadata_env();

        assert_eq!(
            env.get("PIXI_PROJECT_NAME").unwrap(),
            workspace.display_name()
        );
        assert_eq!(
            env.get("PIXI_PROJECT_ROOT").unwrap(),
            workspace.root().to_str().unwrap()
        );
        assert_eq!(
            env.get("PIXI_PROJECT_MANIFEST").unwrap(),
            workspace.workspace.provenance.path.to_str().unwrap()
        );
        assert_eq!(
            env.get("PIXI_PROJECT_VERSION").unwrap(),
            &workspace
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
        [workspace]
        name = "pixi"
        channels = [""]
        platforms = ["linux-64", "osx-64", "win-64"]

        [activation.env]
        ABC = "123test123"
        ZZZ = "123test123"
        ZAB = "123test123"
        "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), project).unwrap();
        let post_activation_env = workspace
            .default_environment()
            .activation_env(Some(Platform::current()));

        // Make sure the user defined environment variables are sorted by input order.
        assert!(
            post_activation_env.keys().position(|key| key == "ABC")
                < post_activation_env.keys().position(|key| key == "ZZZ")
        );
        assert!(
            post_activation_env.keys().position(|key| key == "ZZZ")
                < post_activation_env.keys().position(|key| key == "ZAB")
        );
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
    /// Validates that the activation cache:
    /// - is not written without an install fingerprint marker;
    /// - is written and reused once a marker exists;
    /// - re-runs activation when the fingerprint changes (mimicking
    ///   a re-install with different content).
    #[tokio::test]
    async fn test_run_activation_cache_based_on_install_fingerprint() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = r#"
        [workspace]
        name = "pixi"
        channels = []
        platforms = []

        [activation.env]
        TEST = "ACTIVATION123"
        "#;
        let project =
            Workspace::from_str(temp_dir.path().join("pixi.toml").as_path(), workspace).unwrap();
        let default_env = project.default_environment();

        // Without an install fingerprint, the cache is never written
        // even with experimental=true and a lock file present.
        let env = run_activation(
            &default_env,
            &CurrentEnvVarBehavior::Include,
            Some(&LockFile::default()),
            false,
            true,
        )
        .await
        .unwrap();
        assert!(env.contains_key("CONDA_PREFIX"));
        assert!(!project.activation_env_cache_folder().exists());

        // Write a fingerprint marker so the cache becomes operative.
        pixi_command_dispatcher::EnvironmentFingerprint::from_string("fp-1".to_string())
            .write(&default_env.dir())
            .unwrap();

        let _env = run_activation(
            &default_env,
            &CurrentEnvVarBehavior::Include,
            Some(&LockFile::default()),
            false,
            true,
        )
        .await
        .unwrap();
        assert!(project.activation_env_cache_folder().exists());
        let cache_file = project.default_environment().activation_cache_file_path();
        assert!(cache_file.exists());

        // Hand-edit the cache to confirm the next call returns
        // the stored value rather than re-running activation.
        let contents = tokio_fs::read_to_string(&cache_file).await.unwrap();
        let modified = contents.replace("ACTIVATION123", "ACTIVATION456");
        tokio_fs::write(&cache_file, modified).await.unwrap();
        let env = run_activation(
            &default_env,
            &CurrentEnvVarBehavior::Include,
            Some(&LockFile::default()),
            false,
            true,
        )
        .await
        .unwrap();
        assert_eq!(env.get("TEST").unwrap(), "ACTIVATION456");

        // Bumping the fingerprint mimics a fresh install with
        // different content — the cache key changes, so activation
        // runs again and overwrites the cache file.
        pixi_command_dispatcher::EnvironmentFingerprint::from_string("fp-2".to_string())
            .write(&default_env.dir())
            .unwrap();
        let env = run_activation(
            &default_env,
            &CurrentEnvVarBehavior::Include,
            Some(&LockFile::default()),
            false,
            true,
        )
        .await
        .unwrap();
        assert_eq!(env.get("TEST").unwrap(), "ACTIVATION123");
    }

    /// Activation env vars are part of [`EnvironmentHash::for_activation`]'s
    /// hashed inputs (via `hash_common_inputs`), so adding or
    /// changing one in the manifest invalidates the cache even when
    /// the install fingerprint is unchanged.
    #[tokio::test]
    async fn test_run_activation_cache_based_on_activation_env() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = r#"
        [workspace]
        name = "pixi"
        channels = []
        platforms = []

        [activation.env]
        TEST = "ACTIVATION123"
        "#;
        let project =
            Workspace::from_str(temp_dir.path().join("pixi.toml").as_path(), workspace).unwrap();
        let default_env = project.default_environment();
        // A fingerprint marker is required for the cache to engage at all.
        pixi_command_dispatcher::EnvironmentFingerprint::from_string("fp-stable".to_string())
            .write(&default_env.dir())
            .unwrap();
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
        let workspace = r#"
        [workspace]
        name = "pixi"
        channels = []
        platforms = []

        [activation.env]
        TEST = "ACTIVATION123"
        TEST2 = "ACTIVATION1234"
        "#;
        let project =
            Workspace::from_str(temp_dir.path().join("pixi.toml").as_path(), workspace).unwrap();
        let default_env = project.default_environment();
        // Marker survives the manifest edit (same prefix dir).
        pixi_command_dispatcher::EnvironmentFingerprint::from_string("fp-stable".to_string())
            .write(&default_env.dir())
            .unwrap();
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
}
