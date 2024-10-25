use super::{EnvDir, EnvironmentName, ExposedName, StateChanges};
use crate::{
    global::{BinDir, StateChange},
    prefix::{Executable, Prefix},
};
use fs_err::tokio as tokio_fs;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use miette::IntoDiagnostic;
use once_cell::sync::Lazy;
use pixi_utils::executable_from_path;
use rattler_conda_types::{
    MatchSpec, Matches, PackageName, ParseStrictness, Platform, RepoDataRecord,
};
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::{Shell, ShellEnum},
};
use regex::Regex;
use std::path::Path;
use std::{collections::HashMap, path::PathBuf, str::FromStr};

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
    mut executables: impl Iterator<Item = &'a Executable>,
    bin_dir: &BinDir,
    env_dir: &EnvDir,
) -> miette::Result<ScriptExecMapping> {
    executables
        .find(|executable| executable.name == entry_point)
        .map(|executable| ScriptExecMapping {
            global_script_path: bin_dir.executable_script_path(exposed_name),
            original_executable: executable.path.clone(),
        })
        .ok_or_else(|| {
            miette::miette!(
                "Couldn't find executable {entry_point} in {}, found these executables: {:?}",
                env_dir.path().display(),
                executables.map(|exec| exec.name.clone()).collect_vec()
            )
        })
}

/// Create the environment activation script
pub(crate) fn create_activation_script(
    prefix: &Prefix,
    shell: ShellEnum,
) -> miette::Result<String> {
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
    env_name: &EnvironmentName,
) -> miette::Result<StateChanges> {
    enum AddedOrChanged {
        Unchanged,
        Added,
        Changed,
    }

    let mut state_changes = StateChanges::default();

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
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(global_script_path, std::fs::Permissions::from_mode(0o755))
                .into_diagnostic()?;
        }

        let executable_name = executable_from_path(global_script_path);
        let exposed_name = ExposedName::from_str(&executable_name)?;
        match added_or_changed {
            AddedOrChanged::Unchanged => {}
            AddedOrChanged::Added => {
                state_changes.insert_change(env_name, StateChange::AddedExposed(exposed_name));
            }
            AddedOrChanged::Changed => {
                state_changes.insert_change(env_name, StateChange::UpdatedExposed(exposed_name));
            }
        }
    }
    Ok(state_changes)
}

/// Extracts the executable path from a script file.
///
/// This function reads the content of the script file and attempts to extract
/// the path of the executable it references. It is used to determine
/// the actual binary path from a wrapper script.
pub(crate) async fn extract_executable_from_script(script: &Path) -> miette::Result<PathBuf> {
    // Read the script file into a string
    let script_content = tokio_fs::read_to_string(script).await.into_diagnostic()?;

    // Compile the regex pattern
    #[cfg(unix)]
    const PATTERN: &str = r#""([^"]+)" "\$@""#;
    // The pattern includes `"?` to also find old pixi global installations.
    #[cfg(windows)]
    const PATTERN: &str = r#"@"?([^"]+)"? %/*"#;
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(PATTERN).expect("Failed to compile regex"));

    // Apply the regex to the script content
    if let Some(caps) = RE.captures(&script_content) {
        if let Some(matched) = caps.get(1) {
            return Ok(PathBuf::from(matched.as_str()));
        }
    }
    tracing::debug!(
        "Failed to extract executable path from script {}",
        script_content
    );

    // Return an error if the executable path couldn't be extracted
    miette::bail!(
        "Failed to extract executable path from script {}",
        script.display()
    )
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
    specs: &IndexSet<MatchSpec>,
    platform: Option<Platform>,
) -> bool {
    // Check whether all specs in the manifest are present in the installed
    // environment
    let specs_in_manifest_are_present = specs
        .iter()
        .all(|spec| prefix_records.iter().any(|record| spec.matches(record)));

    if !specs_in_manifest_are_present {
        tracing::debug!("Not all specs in the manifest are present in the environment");
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
            tracing::debug!("Not all packages in the environment have the correct platform");
            return false;
        }
    }

    // Prune dependencies from the repodata that are valid for the requested specs
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
    let remaining_prefix_records = specs.iter().fold(prefix_records, |mut acc, spec| {
        let Some(index) = acc.iter().position(|record| spec.matches(record.as_ref())) else {
            return acc;
        };
        let matched_record = acc.swap_remove(index);
        prune_dependencies(acc, &matched_record)
    });

    // If there are no remaining prefix records, then this means that
    // the environment doesn't contain records that don't match the manifest
    if !remaining_prefix_records.is_empty() {
        tracing::debug!(
            "Environment contains extra entries that don't match the manifest: {:?}",
            remaining_prefix_records
        );
        false
    } else {
        true
    }
}

/// Finds the package name in the prefix and automatically exposes it if an executable is found.
/// This is useful for packages like `ansible` and `jupyter` which don't ship executables their own executables.
/// This function will return the mapping and the package name of the package in which the binary was found.
pub async fn find_binary_by_name(
    prefix: &Prefix,
    package_name: &PackageName,
) -> miette::Result<Option<Executable>> {
    let installed_packages = prefix.find_installed_packages(None).await?;
    for package in &installed_packages {
        let executables = prefix.find_executables(&[package.clone()]);

        // Check if any of the executables match the package name
        if let Some(executable) = executables
            .iter()
            .find(|executable| executable.name.as_str() == package_name.as_normalized())
        {
            return Ok(Some(executable.clone()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use fs_err as fs;
    use rattler_conda_types::{MatchSpec, ParseStrictness, Platform};
    use rattler_lock::LockFile;
    use rstest::{fixture, rstest};

    use super::*;

    #[fixture]
    fn ripgrep_specs() -> IndexSet<MatchSpec> {
        IndexSet::from([MatchSpec::from_str("ripgrep=14.1.0", ParseStrictness::Strict).unwrap()])
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
    fn ripgrep_bat_specs() -> IndexSet<MatchSpec> {
        IndexSet::from([
            MatchSpec::from_str("ripgrep=14.1.0", ParseStrictness::Strict).unwrap(),
            MatchSpec::from_str("bat=0.24.0", ParseStrictness::Strict).unwrap(),
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
        ripgrep_specs: IndexSet<MatchSpec>,
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
        ripgrep_specs: IndexSet<MatchSpec>,
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
        ripgrep_specs: IndexSet<MatchSpec>,
        ripgrep_bat_specs: IndexSet<MatchSpec>,
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
        ripgrep_specs: IndexSet<MatchSpec>,
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
        ripgrep_specs: IndexSet<MatchSpec>,
    ) {
        assert!(
            !local_environment_matches_spec(ripgrep_records, &ripgrep_specs, Some(Platform::Win64),),
            "The record contains linux-64 entries, so the function should always return `false`"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_extract_executable_from_script_windows() {
        let script_without_quote = r#"
@SET "PATH=C:\Users\USER\.pixi/envs\hyperfine\bin:%PATH%"
@SET "CONDA_PREFIX=C:\Users\USER\.pixi/envs\hyperfine"
@C:\Users\USER\.pixi/envs\hyperfine\bin/hyperfine.exe %*
"#;
        let script_path = Path::new("hyperfine.bat");
        let tempdir = tempfile::tempdir().unwrap();
        let script_path = tempdir.path().join(script_path);
        fs::write(&script_path, script_without_quote).unwrap();
        let executable_path = extract_executable_from_script(&script_path).await.unwrap();
        assert_eq!(
            executable_path,
            Path::new("C:\\Users\\USER\\.pixi/envs\\hyperfine\\bin/hyperfine.exe")
        );

        let script_with_quote = r#"
@SET "PATH=C:\Users\USER\.pixi/envs\python\bin;%PATH%"
@SET "CONDA_PREFIX=C:\Users\USER\.pixi/envs\python"
@"C:\Users\USER\.pixi\envs\python\Scripts/pydoc.exe" %*
"#;
        let script_path = Path::new("pydoc.bat");
        let script_path = tempdir.path().join(script_path);
        fs::write(&script_path, script_with_quote).unwrap();
        let executable_path = extract_executable_from_script(&script_path).await.unwrap();
        assert_eq!(
            executable_path,
            Path::new("C:\\Users\\USER\\.pixi\\envs\\python\\Scripts/pydoc.exe")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_extract_executable_from_script_unix() {
        let script = r#"#!/bin/sh
export PATH="/home/user/.pixi/envs/nushell/bin:${PATH}"
export CONDA_PREFIX="/home/user/.pixi/envs/nushell"
"/home/user/.pixi/envs/nushell/bin/nu" "$@"
"#;
        let script_path = Path::new("nu");
        let tempdir = tempfile::tempdir().unwrap();
        let script_path = tempdir.path().join(script_path);
        fs::write(&script_path, script).unwrap();
        let executable_path = extract_executable_from_script(&script_path).await.unwrap();
        assert_eq!(
            executable_path,
            Path::new("/home/user/.pixi/envs/nushell/bin/nu")
        );
    }
}
