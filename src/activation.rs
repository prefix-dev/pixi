use std::collections::HashMap;

use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::Platform;
use rattler_shell::{
    activation::{ActivationError, ActivationVariables, Activator, PathModificationBehavior},
    shell::ShellEnum,
};

use crate::{
    environment::{get_up_to_date_prefix, LockFileUsage},
    progress::await_in_progress,
    project::{manifest::EnvironmentName, Environment},
};

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
async fn run_activation(environment: &Environment<'_>) -> miette::Result<HashMap<String, String>> {
    let activator = get_activator(environment, ShellEnum::default()).map_err(|e| {
        miette::miette!(format!(
            "failed to create activator for {:?}\n{}",
            environment.name(),
            e
        ))
    })?;

    let activator_result = tokio::task::spawn_blocking(move || {
        // Run and cache the activation script
        activator.run_activation(ActivationVariables {
            // Get the current PATH variable
            path: Default::default(),

            // Start from an empty prefix
            conda_prefix: None,

            // Prepending environment paths so they get found first.
            path_modification_behavior: PathModificationBehavior::Prepend,
        })
    })
    .await
    .into_diagnostic()?
    .into_diagnostic()?;

    Ok(activator_result)
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

/// Determine the environment variables that need to be set in an interactive shell to make it
/// function as if the environment has been activated. This method runs the activation scripts from
/// the environment and stores the environment variables it added, finally it adds environment
/// variables from the project.
pub async fn get_activation_env<'p>(
    environment: &'p Environment<'p>,
    lock_file_usage: LockFileUsage,
) -> miette::Result<HashMap<String, String>> {
    // Get the prefix which we can then activate.
    get_up_to_date_prefix(
        environment,
        lock_file_usage,
        false,
        None,
        Default::default(),
    )
    .await?;

    // Get environment variables from the activation
    let activation_env = await_in_progress("activating environment", run_activation(environment))
        .await
        .wrap_err("failed to activate environment")?;

    let environment_variables = get_environment_variables(environment);

    // Construct command environment by concatenating the environments
    Ok(activation_env
        .into_iter()
        .chain(environment_variables.into_iter())
        .collect())
}
