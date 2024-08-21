use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use clap::Parser;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pixi_utils::reqwest::build_reqwest_clients;
use rattler::{
    install::{DefaultProgressFormatter, IndicatifReporter, Installer},
    package_cache::PackageCache,
};
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, PackageName, Platform, PrefixRecord, RepoDataRecord,
};
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::{Shell, ShellEnum},
};
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::VirtualPackage;
use reqwest_middleware::ClientWithMiddleware;

use crate::global::{
    channel_name_from_prefix, find_designated_package, print_executables_available, BinDir,
    BinEnvDir,
};
use crate::{
    cli::cli_config::ChannelsConfig, cli::has_specs::HasSpecs, prefix::Prefix,
    rlimit::try_increase_rlimit_to_sensible,
};
use pixi_config::{self, Config, ConfigCli};
use pixi_progress::{await_in_progress, global_multi_progress, wrap_in_progress};

use super::EnvironmentName;

/// Sync given global environment records with environment on the system
pub(crate) async fn sync_environment(
    environment_name: &EnvironmentName,
    exposed: &IndexMap<String, String>,
    records: Vec<RepoDataRecord>,
    authenticated_client: ClientWithMiddleware,
    platform: Platform,
) -> miette::Result<()> {
    try_increase_rlimit_to_sensible();

    // Create the binary environment prefix where we install or update the package
    let BinEnvDir(bin_prefix) = BinEnvDir::create(environment_name).await?;
    let prefix = Prefix::new(bin_prefix);

    // Install the environment
    let package_cache = PackageCache::new(pixi_config::get_cache_dir()?.join("pkgs"));

    let result = await_in_progress("creating virtual environment", |pb| {
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
            .install(prefix.root(), records)
    })
    .await
    .into_diagnostic()?;

    return Ok(());

    // // Find the installed package in the environment
    // let prefix_package = find_designated_package(&prefix, package_name).await?;

    // // Determine the shell to use for the invocation script
    // let shell: ShellEnum = if cfg!(windows) {
    //     rattler_shell::shell::CmdExe.into()
    // } else {
    //     rattler_shell::shell::Bash.into()
    // };

    // // Construct the reusable activation script for the shell and generate an
    // // invocation script for each executable added by the package to the
    // // environment.
    // let activation_script = create_activation_script(&prefix, shell.clone())?;

    // let bin_dir = BinDir::create().await?;
    // let script_mapping =
    //     find_and_map_executable_scripts(&prefix, &prefix_package, &bin_dir).await?;
    // create_executable_scripts(&script_mapping, &prefix, &shell, activation_script).await?;

    // let scripts: Vec<_> = script_mapping
    //     .into_iter()
    //     .map(
    //         |BinScriptMapping {
    //              global_binary_path: path,
    //              ..
    //          }| path,
    //     )
    //     .collect();

    // Ok((prefix_package, scripts))
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

/// Find all executable scripts in a package and map them to their global
/// install paths.
///
/// (Convenience wrapper around `find_executables` and
/// `map_executables_to_global_bin_scripts` which are generally used together.)
pub(crate) async fn find_and_map_executable_scripts<'a>(
    prefix: &Prefix,
    prefix_package: &'a PrefixRecord,
    bin_dir: &BinDir,
) -> miette::Result<Vec<BinScriptMapping<'a>>> {
    let executables = find_executables(prefix, prefix_package);
    map_executables_to_global_bin_scripts(&executables, bin_dir).await
}

/// Mapping from an executable in a package environment to its global binary
/// script location.
#[derive(Debug)]
pub struct BinScriptMapping<'a> {
    pub original_executable: &'a Path,
    pub global_binary_path: PathBuf,
}

/// Find the executable scripts within the specified package installed in this
/// conda prefix.
fn find_executables<'a>(prefix: &Prefix, prefix_package: &'a PrefixRecord) -> Vec<&'a Path> {
    prefix_package
        .files
        .iter()
        .filter(|relative_path| is_executable(prefix, relative_path))
        .map(|buf| buf.as_ref())
        .collect()
}

fn is_executable(prefix: &Prefix, relative_path: &Path) -> bool {
    // Check if the file is in a known executable directory.
    let binary_folders = if cfg!(windows) {
        &([
            "",
            "Library/mingw-w64/bin/",
            "Library/usr/bin/",
            "Library/bin/",
            "Scripts/",
            "bin/",
        ][..])
    } else {
        &(["bin"][..])
    };

    let parent_folder = match relative_path.parent() {
        Some(dir) => dir,
        None => return false,
    };

    if !binary_folders
        .iter()
        .any(|bin_path| Path::new(bin_path) == parent_folder)
    {
        return false;
    }

    // Check if the file is executable
    let absolute_path = prefix.root().join(relative_path);
    is_executable::is_executable(absolute_path)
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
/// binary script.
async fn map_executables_to_global_bin_scripts<'a>(
    package_executables: &[&'a Path],
    bin_dir: &BinDir,
) -> miette::Result<Vec<BinScriptMapping<'a>>> {
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

    let BinDir(bin_dir) = bin_dir;
    let mut mappings = vec![];

    for exec in package_executables.iter() {
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

        let mut executable_script_path = bin_dir.join(file_name);

        if cfg!(windows) {
            executable_script_path.set_extension("bat");
        };
        mappings.push(BinScriptMapping {
            original_executable: exec,
            global_binary_path: executable_script_path,
        });
    }
    Ok(mappings)
}

/// Create the executable scripts by modifying the activation script
/// to activate the environment and run the executable.
async fn create_executable_scripts(
    mapped_executables: &[BinScriptMapping<'_>],
    prefix: &Prefix,
    shell: &ShellEnum,
    activation_script: String,
) -> miette::Result<()> {
    for BinScriptMapping {
        original_executable: exec,
        global_binary_path: executable_script_path,
    } in mapped_executables
    {
        let mut script = activation_script.clone();
        shell
            .run_command(
                &mut script,
                [
                    format!("\"{}\"", prefix.root().join(exec).to_string_lossy()).as_str(),
                    get_catch_all_arg(shell),
                ],
            )
            .expect("should never fail");

        if matches!(shell, ShellEnum::CmdExe(_)) {
            // wrap the script contents in `@echo off` and `setlocal` to prevent echoing the
            // script and to prevent leaking environment variables into the
            // parent shell (e.g. PATH would grow longer and longer)
            script = format!("@echo off\nsetlocal\n{}\nendlocal", script);
        }

        tokio::fs::write(&executable_script_path, script)
            .await
            .into_diagnostic()?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                executable_script_path,
                std::fs::Permissions::from_mode(0o755),
            )
            .into_diagnostic()?;
        }
    }
    Ok(())
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
