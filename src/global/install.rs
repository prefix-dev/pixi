use std::{
    borrow::Borrow,
    collections::HashMap,
    ffi::OsStr,
    iter,
    path::{Path, PathBuf},
    str::FromStr,
    time,
};

use clap::Parser;
use distribution_types::Diagnostic;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{bail, Context, IntoDiagnostic};
use pixi_config::{self, default_channel_config, Config, ConfigCli};
use pixi_progress::{await_in_progress, global_multi_progress, wrap_in_progress};
use pixi_utils::reqwest::build_reqwest_clients;
use rattler::{
    install::{DefaultProgressFormatter, IndicatifReporter, Installer},
    package_cache::PackageCache,
};
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, Matches, PackageName, ParseStrictness, Platform,
    PrefixRecord, RepoDataRecord,
};
use rattler_repodata_gateway::Gateway;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::{Shell, ShellEnum},
};
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use reqwest_middleware::ClientWithMiddleware;

use super::{common::EnvRoot, project::ParsedEnvironment, EnvironmentName, ExposedKey};
use crate::{
    cli::{cli_config::ChannelsConfig, has_specs::HasSpecs, project::platform},
    global::{self, channel_name_from_prefix, find_designated_package, BinDir, EnvDir},
    prefix::Prefix,
    rlimit::try_increase_rlimit_to_sensible,
    task::ExecutableTask,
};

/// Installs global environment records
pub(crate) async fn install_environment(
    specs: &IndexMap<PackageName, MatchSpec>,
    env_name: &EnvironmentName,
    parsed_environment: &ParsedEnvironment,
    authenticated_client: ClientWithMiddleware,
    prefix: &Prefix,
    config: &Config,
    gateway: &Gateway,
) -> miette::Result<()> {
    let channels = parsed_environment
        .channels()
        .into_iter()
        .map(|channel| channel.clone().into_channel(config.global_channel_config()))
        .collect_vec();

    let platform = parsed_environment
        .platform()
        .unwrap_or_else(Platform::current);

    let repodata = await_in_progress("querying repodata ", |_| async {
        gateway
            .query(
                channels,
                [platform, Platform::NoArch],
                specs.values().cloned().collect_vec(),
            )
            .recursive(true)
            .await
            .into_diagnostic()
    })
    .await?;

    // Determine virtual packages of the current platform
    let virtual_packages = VirtualPackage::detect(&VirtualPackageOverrides::default())
        .into_diagnostic()
        .context("failed to determine virtual packages")?
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .collect();

    // Solve the environment
    let solver_specs = specs.clone();
    let solved_records = tokio::task::spawn_blocking(move || {
        wrap_in_progress("solving environment", move || {
            Solver.solve(SolverTask {
                specs: solver_specs.values().cloned().collect_vec(),
                virtual_packages,
                ..SolverTask::from_iter(&repodata)
            })
        })
        .into_diagnostic()
        .context("failed to solve environment")
    })
    .await
    .into_diagnostic()??;

    try_increase_rlimit_to_sensible();

    // Install the environment
    let package_cache = PackageCache::new(pixi_config::get_cache_dir()?.join("pkgs"));

    await_in_progress("creating virtual environment", |pb| {
        Installer::new()
            .with_download_client(authenticated_client)
            .with_io_concurrency_limit(100)
            .with_execute_link_scripts(false)
            .with_package_cache(package_cache)
            .with_target_platform(platform)
            .with_reporter(
                IndicatifReporter::builder()
                    .with_multi_progress(global_multi_progress())
                    .with_placement(rattler::install::Placement::After(pb))
                    .with_formatter(DefaultProgressFormatter::default().with_prefix("  "))
                    .clear_when_done(true)
                    .finish(),
            )
            .install(prefix.root(), solved_records)
    })
    .await
    .into_diagnostic()?;

    Ok(())
}

pub(crate) async fn expose_executables(
    env_name: &EnvironmentName,
    parsed_environment: &ParsedEnvironment,
    packages: Vec<PackageName>,
    prefix: &Prefix,
    bin_dir: &BinDir,
) -> miette::Result<bool> {
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

    // Processes prefix records to filter and collect executable files.
    let executables: Vec<(String, PathBuf)> = prefix_records
        .into_iter()
        // Filters records to only include direct dependencies
        .filter(|record| packages.contains(&record.repodata_record.package_record.name))
        // Finds executables for each filtered record.
        .flat_map(|record| global::find_executables(prefix, &record))
        // Maps executables to a tuple of file name (as a string) and file path.
        .filter_map(|path| {
            path.file_stem()
                .and_then(OsStr::to_str)
                .map(|name| (name.to_string(), path.clone()))
        })
        // Filters tuples to include only those whose names are in the `exposed` values
        .filter(|(name, path)| parsed_environment.exposed.values().contains(&name))
        .collect();

    let script_mapping = parsed_environment
        .exposed
        .iter()
        .map(|(exposed_name, entry_point)| {
            script_exec_mapping(
                exposed_name,
                entry_point,
                executables.clone(),
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
/// Returns an error if the entry point is not found in the list of executable
/// names.
fn script_exec_mapping(
    exposed_name: &ExposedKey,
    entry_point: &str,
    executables: impl IntoIterator<Item = (String, PathBuf)>,
    bin_dir: &BinDir,
    environment_name: &EnvironmentName,
) -> miette::Result<ScriptExecMapping> {
    executables
        .into_iter()
        .find(|(executable_name, _)| *executable_name == entry_point)
        .map(|(_, executable_path)| ScriptExecMapping {
            global_script_path: bin_dir.executable_script_path(exposed_name),
            original_executable: executable_path,
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

/// For each executable provided, map it to the installation path for its global
/// executable script.
async fn map_executables_to_global_bin_scripts(
    package_executables: impl IntoIterator<Item = PathBuf>,
    bin_dir: &BinDir,
) -> miette::Result<Vec<ScriptExecMapping>> {
    #[cfg(target_family = "windows")]
    let extensions_list: Vec<String> = if let Ok(pathext) = std::env::var("PATHEXT") {
        pathext.split(';').map(|s| s.to_lowercase()).collect()
    } else {
        tracing::debug!("Could not find 'PATHEXT' variable, using a default list");
        [
            ".COM", ".EXE", ".BAT", ".CMD", ".VBS", ".VBE", ".JS", ".JSE", ".WSF", ".WSH", ".MSC",
            ".CPL",
        ]
        .iter()
        .map(|&s| s.to_lowercase())
        .collect()
    };

    #[cfg(target_family = "unix")]
    // TODO: Find if there are more relevant cases, these cases are generated by our big friend
    // GPT-4
    let extensions_list: Vec<String> = vec![
        ".sh", ".bash", ".zsh", ".csh", ".tcsh", ".ksh", ".fish", ".py", ".pl", ".rb", ".lua",
        ".php", ".tcl", ".awk", ".sed",
    ]
    .iter()
    .map(|&s| s.to_owned())
    .collect();

    let mut mappings = vec![];

    for exec in package_executables {
        // Remove the extension of a file if it is in the list of known extensions.
        let Some(file_name) = exec
            .file_name()
            .and_then(OsStr::to_str)
            .map(str::to_lowercase)
        else {
            continue;
        };
        let file_name = extensions_list
            .iter()
            .find_map(|ext| file_name.strip_suffix(ext))
            .unwrap_or(file_name.as_str());

        let mut executable_script_path = bin_dir.path().join(file_name);

        if cfg!(windows) {
            executable_script_path.set_extension("bat");
        };
        mappings.push(ScriptExecMapping {
            original_executable: exec,
            global_script_path: executable_script_path,
        });
    }
    Ok(mappings)
}

/// Create the executable scripts by modifying the activation script
/// to activate the environment and run the executable.
///
/// Returns true if a change was made.
async fn create_executable_scripts(
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
            match tokio::fs::read_to_string(global_script_path).await {
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
            tokio::fs::write(&global_script_path, script)
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

        let executable_name = global_script_path
            .file_stem()
            .and_then(OsStr::to_str)
            .expect("must always have at least a name");
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

pub(crate) async fn sync(config: &Config, assume_yes: bool) -> Result<(), miette::Error> {
    // Create directories
    let bin_dir = BinDir::from_env().await?;
    let env_root = EnvRoot::from_env().await?;

    let project = global::Project::discover_or_create(&bin_dir, &env_root, assume_yes)
        .await?
        .with_cli_config(config.clone());

    // Fetch the repodata
    let (_, auth_client) = build_reqwest_clients(Some(config));

    let gateway = config.gateway(auth_client.clone());

    // Prune environments that are not listed
    env_root
        .prune(project.environments().keys().cloned())
        .await?;

    // Remove binaries that are not listed as exposed
    let exposed_paths = project
        .environments()
        .values()
        .flat_map(|environment| {
            environment
                .exposed
                .keys()
                .map(|e| bin_dir.executable_script_path(e))
        })
        .collect_vec();
    for file in bin_dir.files().await? {
        let file_name = file
            .file_stem()
            .and_then(OsStr::to_str)
            .ok_or_else(|| miette::miette!("Could not get file stem of {}", file.display()))?;
        if !exposed_paths.contains(&file) && file_name != "pixi" {
            tokio::fs::remove_file(&file)
                .await
                .into_diagnostic()
                .wrap_err_with(|| format!("Could not remove {}", &file.display()))?;
            eprintln!(
                "{}Remove executable '{file_name}'.",
                console::style(console::Emoji("✔ ", "")).green()
            );
        }
    }

    for (env_name, parsed_environment) in project.environments() {
        let specs = parsed_environment
            .dependencies
            .clone()
            .into_iter()
            .map(|(name, spec)| {
                let match_spec = MatchSpec::from_nameless(
                    spec.clone()
                        .try_into_nameless_match_spec(&default_channel_config())
                        .into_diagnostic()?
                        .ok_or_else(|| {
                            miette::miette!("Could not convert {spec:?} to nameless match spec.")
                        })?,
                    Some(name.clone()),
                );
                Ok((name, match_spec))
            })
            .collect::<Result<IndexMap<PackageName, MatchSpec>, miette::Report>>()?;

        let env_dir = EnvDir::new(env_root.clone(), env_name.clone()).await?;
        let prefix = Prefix::new(env_dir.path());

        let prefix_records = prefix.find_installed_packages(Some(50)).await?;

        if !specs_match_local_environment(&specs, prefix_records, parsed_environment.platform()) {
            install_environment(
                &specs,
                &env_name,
                &parsed_environment,
                auth_client.clone(),
                &prefix,
                config,
                &gateway,
            )
            .await?;
        }

        expose_executables(
            &env_name,
            &parsed_environment,
            specs.keys().cloned().collect(),
            &prefix,
            &bin_dir,
        )
        .await?;
    }

    Ok(())
}

/// Checks if the local environment matches the given specifications.
///
/// This function verifies that all the given specifications are present in the
/// local environment's prefix records and that there are no extra entries in
/// the prefix records that do not match any of the specifications.
fn specs_match_local_environment<T: AsRef<RepoDataRecord>>(
    specs: &IndexMap<PackageName, MatchSpec>,
    prefix_records: Vec<T>,
    platform: Option<Platform>,
) -> bool {
    // Check whether all specs in the manifest are present in the installed
    // environment
    let specs_in_manifest_are_present = specs.iter().all(|(name, spec)| {
        prefix_records
            .iter()
            .any(|record| spec.matches(record.as_ref()))
    });

    if !specs_in_manifest_are_present {
        return false;
    }

    // Check whether all packages in the installed environment have the correct
    // platform
    let platform_specs_match_env = prefix_records.iter().all(|record| {
        let Ok(package_platform) = Platform::from_str(&record.as_ref().package_record.subdir)
        else {
            return true;
        };

        match package_platform {
            Platform::NoArch => true,
            p if Some(p) == platform => true,
            _ => false,
        }
    });

    if !platform_specs_match_env {
        return false;
    }

    fn prune_dependencies<T: AsRef<RepoDataRecord>>(
        mut remaining_prefix_records: Vec<T>,
        matched_record: &T,
    ) -> Vec<T> {
        let mut work_queue = Vec::from([matched_record.as_ref().clone()]);

        while let Some(current_record) = work_queue.pop() {
            let dependencies = &current_record.as_ref().depends;
            for dependency in dependencies {
                let Ok(match_spec) = MatchSpec::from_str(dependency, ParseStrictness::Lenient)
                else {
                    continue;
                };
                let Some(index) = remaining_prefix_records
                    .iter()
                    .position(|record| match_spec.matches(&record.as_ref().package_record))
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
            record.as_ref().package_record.name == *name && spec.matches(record.as_ref())
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
    use rattler_lock::Package;
    use url::Url;

    use super::*;

    #[test]
    fn test_specs_match_local_environment() {
        // Define some test data
        let mut specs = IndexMap::new();
        specs.insert(
            PackageName::from_str("life_package").unwrap(),
            MatchSpec::from_str("life_package=1.2.7", ParseStrictness::Strict).unwrap(),
        );

        let foo_bar_package = Package::build("foo-bar-car", "2").finish();

        let repo_data_records = vec![RepoDataRecord {
            package_record: foo_bar_package.package_record,
            file_name: "foo-bar-car".to_owned(),
            url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
            channel: "dummy-channel".to_owned(),
        }];

        assert!(specs_match_local_environment(
            &specs,
            repo_data_records,
            None
        ));
    }
}
