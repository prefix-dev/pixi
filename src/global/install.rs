use fs_err::tokio as tokio_fs;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_utils::executable_from_path;
use pixi_utils::reqwest::build_reqwest_clients;
use rattler_conda_types::{MatchSpec, Matches, PackageName, ParseStrictness, Platform, RepoDataRecord};
use rattler_repodata_gateway::Gateway;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::{Shell, ShellEnum},
};
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use reqwest_middleware::ClientWithMiddleware;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    str::FromStr,
};

use super::{project::ParsedEnvironment, EnvironmentName, ExposedName, Project};
use crate::{
    global::{self, BinDir, EnvDir},
    prefix::Prefix,
    rlimit::try_increase_rlimit_to_sensible,
};
use crate::repodata::Repodata;

pub(crate) async fn expose_executables(
    env_name: &EnvironmentName,
    parsed_environment: &ParsedEnvironment,
    prefix: &Prefix,
    bin_dir: &BinDir,
) -> miette::Result<bool> {
    tracing::debug!("Exposing executables for environment '{}'", env_name);
    // Determine the shell to use for the invocation script
    let shell: ShellEnum = if cfg!(windows) {
        rattler_shell::shell::CmdExe.into()
    } else {
        rattler_shell::shell::Bash.into()
    };

    // Construct the reusable activation script for the shell and generate an
    // invocation script for each executable added by the package to the
    // environment.
    let activation_script = create_activation_script(prefix, shell.clone())?;

    let prefix_records = prefix.find_installed_packages(None).await?;

    let all_executables = prefix.find_executables(prefix_records.as_slice());

    let exposed: HashSet<&String> = parsed_environment.exposed.values().collect();

    let exposed_executables: Vec<_> = all_executables
        .into_iter()
        .filter(|(name, _)| exposed.contains(name))
        .collect();

    let script_mapping = parsed_environment
        .exposed
        .iter()
        .map(|(exposed_name, entry_point)| {
            script_exec_mapping(
                exposed_name,
                entry_point,
                exposed_executables.iter(),
                bin_dir,
                env_name,
            )
        })
        .collect::<miette::Result<Vec<_>>>()?;

    create_executable_scripts(&script_mapping, prefix, &shell, activation_script).await
}

/// Maps an entry point in the environment to a concrete `ScriptExecMapping`.
///
/// This function takes an entry point and a list of executable names and paths,
/// and returns a `ScriptExecMapping` that contains the path to the script and
/// the original executable.
/// # Returns
///
/// A `miette::Result` containing the `ScriptExecMapping` if the entry point is
/// found, or an error if it is not.
///
/// # Errors
///
/// Returns an error if the entry point is not found in the list of executable names.
pub(crate) fn script_exec_mapping<'a>(
    exposed_name: &ExposedName,
    entry_point: &str,
    mut executables: impl Iterator<Item = &'a (String, PathBuf)>,
    bin_dir: &BinDir,
    environment_name: &EnvironmentName,
) -> miette::Result<ScriptExecMapping> {
    executables
        .find(|(executable_name, _)| *executable_name == entry_point)
        .map(|(_, executable_path)| ScriptExecMapping {
            global_script_path: bin_dir.executable_script_path(exposed_name),
            original_executable: executable_path.clone(),
        })
        .ok_or_else(|| miette::miette!("Could not find {entry_point} in {environment_name}"))
}

/// Create the environment activation script
fn create_activation_script(prefix: &Prefix, shell: ShellEnum) -> miette::Result<String> {
    let activator =
        Activator::from_path(prefix.root(), shell, Platform::current()).into_diagnostic()?;
    let result = activator
        .activation(ActivationVariables {
            conda_prefix: None,
            path: None,
            path_modification_behavior: PathModificationBehavior::Prepend,
        })
        .into_diagnostic()?;

    // Add a shebang on unix based platforms
    let script = if cfg!(unix) {
        format!("#!/bin/sh\n{}", result.script.contents().into_diagnostic()?)
    } else {
        result.script.contents().into_diagnostic()?
    };

    Ok(script)
}

/// Mapping from the global script location to an executable in a package
/// environment .
#[derive(Debug)]
pub struct ScriptExecMapping {
    pub global_script_path: PathBuf,
    pub original_executable: PathBuf,
}

/// Returns the string to add for all arguments passed to the script
fn get_catch_all_arg(shell: &ShellEnum) -> &str {
    match shell {
        ShellEnum::CmdExe(_) => "%*",
        ShellEnum::PowerShell(_) => "@args",
        _ => "\"$@\"",
    }
}


/// Create the executable scripts by modifying the activation script
/// to activate the environment and run the executable.
pub(crate) async fn create_executable_scripts(
    mapped_executables: &[ScriptExecMapping],
    prefix: &Prefix,
    shell: &ShellEnum,
    activation_script: String,
) -> miette::Result<bool> {
    let mut changed = false;
    enum AddedOrChanged {
        Unchanged,
        Added,
        Changed,
    }

    for ScriptExecMapping {
        global_script_path,
        original_executable,
    } in mapped_executables
    {
        let mut script = activation_script.clone();
        shell
            .run_command(
                &mut script,
                [
                    format!(
                        "\"{}\"",
                        prefix.root().join(original_executable).to_string_lossy()
                    )
                    .as_str(),
                    get_catch_all_arg(shell),
                ],
            )
            .expect("should never fail");

        if matches!(shell, ShellEnum::CmdExe(_)) {
            // wrap the script contents in `@echo off` and `setlocal` to prevent echoing the
            // script and to prevent leaking environment variables into the
            // parent shell (e.g. PATH would grow longer and longer)
            script = format!(
                "@echo off\nsetlocal\n{}\nset exitcode=%ERRORLEVEL%\nendlocal\nexit %exitcode%",
                script.trim()
            );
        }

        let added_or_changed = if global_script_path.exists() {
            match tokio_fs::read_to_string(global_script_path).await {
                Ok(previous_script) if previous_script != script => AddedOrChanged::Changed,
                Ok(_) => AddedOrChanged::Unchanged,
                Err(_) => AddedOrChanged::Changed,
            }
        } else {
            AddedOrChanged::Added
        };

        if matches!(
            added_or_changed,
            AddedOrChanged::Changed | AddedOrChanged::Added
        ) {
            tokio_fs::write(&global_script_path, script)
                .await
                .into_diagnostic()?;
            changed = true;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(global_script_path, std::fs::Permissions::from_mode(0o755))
                .into_diagnostic()?;
        }

        let executable_name = executable_from_path(global_script_path);
        match added_or_changed {
            AddedOrChanged::Unchanged => {}
            AddedOrChanged::Added => eprintln!(
                "{}Added executable '{}'.",
                console::style(console::Emoji("✔ ", "")).green(),
                executable_name
            ),
            AddedOrChanged::Changed => eprintln!(
                "{}Updated executable '{}'.",
                console::style(console::Emoji("~ ", "")).yellow(),
                executable_name
            ),
        }
    }
    Ok(changed)
}

/// Warn user on dangerous package installations, interactive yes no prompt
#[allow(unused)]
pub(crate) fn prompt_user_to_continue(
    packages: &IndexMap<PackageName, MatchSpec>,
) -> miette::Result<bool> {
    let dangerous_packages = HashMap::from([
        ("pixi", "Installing `pixi` globally doesn't work as expected.\nUse `pixi self-update` to update pixi and `pixi self-update --version x.y.z` for a specific version."),
        ("pip", "Installing `pip` with `pixi global` won't make pip-installed packages globally available.\nInstead, use a pixi project and add PyPI packages with `pixi add --pypi`, which is recommended. Alternatively, `pixi add pip` and use it within the project.")
    ]);

    // Check if any of the packages are dangerous, and prompt the user to ask if
    // they want to continue, including the advice.
    for (name, _spec) in packages {
        if let Some(advice) = dangerous_packages.get(&name.as_normalized()) {
            let prompt = format!(
                "{}\nDo you want to continue?",
                console::style(advice).yellow()
            );
            if !dialoguer::Confirm::new()
                .with_prompt(prompt)
                .default(false)
                .show_default(true)
                .interact()
                .into_diagnostic()?
            {
                return Ok(false);
            }
        }
    }

    Ok(true)
}

/// Checks if the local environment matches the given specifications.
///
/// This function verifies that all the given specifications are present in the
/// local environment's prefix records and that there are no extra entries in
/// the prefix records that do not match any of the specifications.
pub(crate) fn local_environment_matches_spec(
    prefix_records: Vec<RepoDataRecord>,
    specs: &IndexMap<PackageName, MatchSpec>,
    platform: Option<Platform>,
) -> bool {
    // Check whether all specs in the manifest are present in the installed
    // environment
    let specs_in_manifest_are_present = specs
        .values()
        .all(|spec| prefix_records.iter().any(|record| spec.matches(record)));

    if !specs_in_manifest_are_present {
        return false;
    }

    // Check whether all packages in the installed environment have the correct
    // platform
    if let Some(platform) = platform {
        let platform_specs_match_env = prefix_records.iter().all(|record| {
            let Ok(package_platform) = Platform::from_str(&record.package_record.subdir) else {
                return true;
            };

            match package_platform {
                Platform::NoArch => true,
                p if p == platform => true,
                _ => false,
            }
        });

        if !platform_specs_match_env {
            return false;
        }
    }

    fn prune_dependencies(
        mut remaining_prefix_records: Vec<RepoDataRecord>,
        matched_record: &RepoDataRecord,
    ) -> Vec<RepoDataRecord> {
        let mut work_queue = Vec::from([matched_record.as_ref().clone()]);

        while let Some(current_record) = work_queue.pop() {
            let dependencies = &current_record.depends;
            for dependency in dependencies {
                let Ok(match_spec) = MatchSpec::from_str(dependency, ParseStrictness::Lenient)
                else {
                    continue;
                };
                let Some(index) = remaining_prefix_records
                    .iter()
                    .position(|record| match_spec.matches(&record.package_record))
                else {
                    continue;
                };

                let matched_record = remaining_prefix_records.remove(index).as_ref().clone();
                work_queue.push(matched_record);
            }
        }

        remaining_prefix_records
    }

    // Process each spec and remove matched entries and their dependencies
    let remaining_prefix_records = specs.iter().fold(prefix_records, |mut acc, (name, spec)| {
        let Some(index) = acc.iter().position(|record| {
            record.package_record.name == *name && spec.matches(record.as_ref())
        }) else {
            return acc;
        };
        let matched_record = acc.swap_remove(index);
        prune_dependencies(acc, &matched_record)
    });

    // If there are no remaining prefix records, then this means that
    // the environment doesn't contain records that don't match the manifest
    remaining_prefix_records.is_empty()
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use rattler_conda_types::{MatchSpec, PackageName, ParseStrictness, Platform};
    use rattler_lock::LockFile;
    use rstest::{fixture, rstest};

    use super::*;

    #[fixture]
    fn ripgrep_specs() -> IndexMap<PackageName, MatchSpec> {
        IndexMap::from([(
            PackageName::from_str("ripgrep").unwrap(),
            MatchSpec::from_str("ripgrep=14.1.0", ParseStrictness::Strict).unwrap(),
        )])
    }

    #[fixture]
    fn ripgrep_records() -> Vec<RepoDataRecord> {
        LockFile::from_str(include_str!("./test_data/lockfiles/ripgrep.lock"))
            .unwrap()
            .default_environment()
            .unwrap()
            .conda_repodata_records_for_platform(Platform::Linux64)
            .unwrap()
            .unwrap()
    }

    #[fixture]
    fn ripgrep_bat_specs() -> IndexMap<PackageName, MatchSpec> {
        IndexMap::from([
            (
                PackageName::from_str("ripgrep").unwrap(),
                MatchSpec::from_str("ripgrep=14.1.0", ParseStrictness::Strict).unwrap(),
            ),
            (
                PackageName::from_str("bat").unwrap(),
                MatchSpec::from_str("bat=0.24.0", ParseStrictness::Strict).unwrap(),
            ),
        ])
    }

    #[fixture]
    fn ripgrep_bat_records() -> Vec<RepoDataRecord> {
        LockFile::from_str(include_str!("./test_data/lockfiles/ripgrep_bat.lock"))
            .unwrap()
            .default_environment()
            .unwrap()
            .conda_repodata_records_for_platform(Platform::Linux64)
            .unwrap()
            .unwrap()
    }

    #[rstest]
    fn test_local_environment_matches_spec(
        ripgrep_records: Vec<RepoDataRecord>,
        ripgrep_specs: IndexMap<PackageName, MatchSpec>,
    ) {
        assert!(local_environment_matches_spec(
            ripgrep_records,
            &ripgrep_specs,
            None
        ));
    }

    #[rstest]
    fn test_local_environment_misses_entries_for_specs(
        mut ripgrep_records: Vec<RepoDataRecord>,
        ripgrep_specs: IndexMap<PackageName, MatchSpec>,
    ) {
        // Remove last repodata record
        ripgrep_records.pop();

        assert!(!local_environment_matches_spec(
            ripgrep_records,
            &ripgrep_specs,
            None
        ));
    }

    #[rstest]
    fn test_local_environment_has_too_many_entries_to_match_spec(
        ripgrep_bat_records: Vec<RepoDataRecord>,
        ripgrep_specs: IndexMap<PackageName, MatchSpec>,
        ripgrep_bat_specs: IndexMap<PackageName, MatchSpec>,
    ) {
        assert!(!local_environment_matches_spec(
            ripgrep_bat_records.clone(),
            &ripgrep_specs,
            None
        ), "The function needs to detect that records coming from ripgrep and bat don't match ripgrep alone.");

        assert!(
            local_environment_matches_spec(ripgrep_bat_records, &ripgrep_bat_specs, None),
            "The records and specs match and the function should return `true`."
        );
    }

    #[rstest]
    fn test_local_environment_matches_given_platform(
        ripgrep_records: Vec<RepoDataRecord>,
        ripgrep_specs: IndexMap<PackageName, MatchSpec>,
    ) {
        assert!(
            local_environment_matches_spec(
                ripgrep_records,
                &ripgrep_specs,
                Some(Platform::Linux64)
            ),
            "The records contains only linux-64 entries"
        );
    }

    #[rstest]
    fn test_local_environment_doesnt_match_given_platform(
        ripgrep_records: Vec<RepoDataRecord>,
        ripgrep_specs: IndexMap<PackageName, MatchSpec>,
    ) {
        assert!(
            !local_environment_matches_spec(ripgrep_records, &ripgrep_specs, Some(Platform::Win64),),
            "The record contains linux-64 entries, so the function should always return `false`"
        );
    }
}
