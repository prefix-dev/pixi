use crate::install::execute_transaction;
use crate::repodata::friendly_channel_name;
use crate::{config, prefix::Prefix, progress::await_in_progress, repodata::fetch_sparse_repodata};
use clap::Parser;
use dirs::home_dir;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler::install::Transaction;
use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, PackageName, Platform, PrefixRecord};
use rattler_networking::AuthenticatedClient;
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::Shell,
    shell::ShellEnum,
};
use rattler_solve::{resolvo, SolverImpl};
use std::ffi::OsStr;
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

/// Installs the defined package in a global accessible location.
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the package that is to be installed.
    package: String,

    /// Represents the channels from which the package will be installed.
    /// Multiple channels can be specified by using this field multiple times.
    ///
    /// When specifying a channel, it is common that the selected channel also
    /// depends on the `conda-forge` channel.
    /// For example: `pixi global install --channel conda-forge --channel bioconda`.
    ///
    /// By default, if no channel is provided, `conda-forge` is used.
    #[clap(short, long, default_values = ["conda-forge"])]
    channel: Vec<String>,
}

pub(crate) struct BinDir(pub PathBuf);

impl BinDir {
    /// Create the Binary Executable directory
    pub async fn create() -> miette::Result<Self> {
        let bin_dir = bin_dir()?;
        tokio::fs::create_dir_all(&bin_dir)
            .await
            .into_diagnostic()?;
        Ok(Self(bin_dir))
    }

    /// Get the Binary Executable directory, erroring if it doesn't already exist.
    pub async fn from_existing() -> miette::Result<Self> {
        let bin_dir = bin_dir()?;
        if tokio::fs::try_exists(&bin_dir).await.into_diagnostic()? {
            Ok(Self(bin_dir))
        } else {
            Err(miette::miette!(
                "binary executable directory does not exist"
            ))
        }
    }
}

/// Get pixi home directory, default to `$HOME/.pixi`
fn home_path() -> miette::Result<PathBuf> {
    if let Some(path) = std::env::var_os("PIXI_HOME") {
        Ok(PathBuf::from(path))
    } else {
        home_dir()
            .map(|path| path.join(".pixi"))
            .ok_or_else(|| miette::miette!("could not find home directory"))
    }
}

/// Global binaries directory, default to `$HOME/.pixi/bin`
fn bin_dir() -> miette::Result<PathBuf> {
    home_path().map(|path| path.join("bin"))
}

pub(crate) struct BinEnvDir(pub PathBuf);

impl BinEnvDir {
    /// Construct the path to the env directory for the binary package `package_name`.
    fn package_bin_env_dir(package_name: &PackageName) -> miette::Result<PathBuf> {
        Ok(bin_env_dir()?.join(package_name.as_normalized()))
    }

    /// Get the Binary Environment directory, erroring if it doesn't already exist.
    pub async fn from_existing(package_name: &PackageName) -> miette::Result<Self> {
        let bin_env_dir = Self::package_bin_env_dir(package_name)?;
        if tokio::fs::try_exists(&bin_env_dir)
            .await
            .into_diagnostic()?
        {
            Ok(Self(bin_env_dir))
        } else {
            Err(miette::miette!(
                "could not find environment for package {}",
                package_name.as_source()
            ))
        }
    }

    /// Create the Binary Environment directory
    pub async fn create(package_name: &PackageName) -> miette::Result<Self> {
        let bin_env_dir = Self::package_bin_env_dir(package_name)?;
        tokio::fs::create_dir_all(&bin_env_dir)
            .await
            .into_diagnostic()?;
        Ok(Self(bin_env_dir))
    }
}

/// GLobal binary environments directory, default to `$HOME/.pixi/envs`
pub(crate) fn bin_env_dir() -> miette::Result<PathBuf> {
    home_path().map(|path| path.join("envs"))
}

/// Find the designated package in the prefix
pub(crate) async fn find_designated_package(
    prefix: &Prefix,
    package_name: &PackageName,
) -> miette::Result<PrefixRecord> {
    let prefix_records = prefix.find_installed_packages(None).await?;
    prefix_records
        .into_iter()
        .find(|r| r.repodata_record.package_record.name == *package_name)
        .ok_or_else(|| miette::miette!("could not find {} in prefix", package_name.as_source()))
}

/// Create the environment activation script
pub(crate) fn create_activation_script(
    prefix: &Prefix,
    shell: ShellEnum,
) -> miette::Result<String> {
    let activator =
        Activator::from_path(prefix.root(), shell, Platform::Osx64).into_diagnostic()?;
    let result = activator
        .activation(ActivationVariables {
            conda_prefix: None,
            path: None,
            path_modification_behavior: PathModificationBehavior::Prepend,
        })
        .into_diagnostic()?;

    // Add a shebang on unix based platforms
    let script = if cfg!(unix) {
        format!("#!/bin/sh\n{}", result.script)
    } else {
        result.script
    };

    Ok(script)
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

/// Find the executable scripts within the specified package installed in this conda prefix.
fn find_executables<'a>(prefix: &Prefix, prefix_package: &'a PrefixRecord) -> Vec<&'a Path> {
    prefix_package
        .files
        .iter()
        .filter(|relative_path| is_executable(prefix, relative_path))
        .map(|buf| buf.as_ref())
        .collect()
}

/// Mapping from an executable in a package environment to its global binary script location.
#[derive(Debug)]
pub(crate) struct BinScriptMapping<'a> {
    pub original_executable: &'a Path,
    pub global_binary_path: PathBuf,
}

/// For each executable provided, map it to the installation path for its global binary script.
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
    // TODO: Find if there are more relevant cases, these cases are generated by our big friend GPT-4
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

/// Find all executable scripts in a package and map them to their global install paths.
///
/// (Convenience wrapper around `find_executables` and `map_executables_to_global_bin_scripts` which
/// are generally used together.)
pub(crate) async fn find_and_map_executable_scripts<'a>(
    prefix: &Prefix,
    prefix_package: &'a PrefixRecord,
    bin_dir: &BinDir,
) -> miette::Result<Vec<BinScriptMapping<'a>>> {
    let executables = find_executables(prefix, prefix_package);
    map_executables_to_global_bin_scripts(&executables, bin_dir).await
}

/// Create the executable scripts by modifying the activation script
/// to activate the environment and run the executable.
pub(crate) async fn create_executable_scripts(
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
                    format!(r###""{}""###, prefix.root().join(exec).to_string_lossy()).as_str(),
                    get_catch_all_arg(shell),
                ],
            )
            .expect("should never fail");
        tokio::fs::write(&executable_script_path, script)
            .await
            .into_diagnostic()?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                executable_script_path,
                std::fs::Permissions::from_mode(0o744),
            )
            .into_diagnostic()?;
        }
    }
    Ok(())
}

/// Install a global command
pub async fn execute(args: Args) -> miette::Result<()> {
    // Figure out what channels we are using
    let channel_config = ChannelConfig::default();
    let channels = args
        .channel
        .iter()
        .map(|c| Channel::from_str(c, &channel_config))
        .collect::<Result<Vec<Channel>, _>>()
        .into_diagnostic()?;
    let authenticated_client = AuthenticatedClient::default();

    // Find the MatchSpec we want to install
    let package_matchspec = MatchSpec::from_str(&args.package).into_diagnostic()?;

    // Fetch sparse repodata
    let platform_sparse_repodata =
        fetch_sparse_repodata(&channels, [Platform::current()], &authenticated_client).await?;

    // Install the package
    let (prefix_package, scripts, _) = globally_install_package(
        package_matchspec,
        &platform_sparse_repodata,
        &channel_config,
        authenticated_client,
    )
    .await?;

    let channel_name = channel_name_from_prefix(&prefix_package, &channel_config);
    let whitespace = console::Emoji("  ", "").to_string();

    eprintln!(
        "{}Installed package {} {} {} from {}",
        console::style(console::Emoji("✔ ", "")).green(),
        console::style(
            prefix_package
                .repodata_record
                .package_record
                .name
                .as_source()
        )
        .bold(),
        console::style(prefix_package.repodata_record.package_record.version).bold(),
        console::style(prefix_package.repodata_record.package_record.build).bold(),
        channel_name,
    );

    let BinDir(bin_dir) = BinDir::from_existing().await?;
    let script_names = scripts
        .into_iter()
        .map(|path| {
            path.strip_prefix(&bin_dir)
                .expect("script paths were constructed by joining onto BinDir")
                .to_string_lossy()
                .to_string()
        })
        .join(&format!("\n{whitespace} -  "));

    if is_bin_folder_on_path() {
        eprintln!(
            "{whitespace}These apps are now globally available:\n{whitespace} -  {script_names}",
        )
    } else {
        eprintln!("{whitespace}These apps have been added to {}\n{whitespace} -  {script_names}\n\n{} To use them, make sure to add {} to your PATH",
                      console::style(&bin_dir.display()).bold(),
                      console::style("!").yellow().bold(),
                      console::style(&bin_dir.display()).bold()
            )
    }

    Ok(())
}

pub(super) async fn globally_install_package(
    package_matchspec: MatchSpec,
    platform_sparse_repodata: &[SparseRepoData],
    channel_config: &ChannelConfig,
    authenticated_client: AuthenticatedClient,
) -> miette::Result<(PrefixRecord, Vec<PathBuf>, bool)> {
    let package_name = package_name(&package_matchspec)?;

    let available_packages = SparseRepoData::load_records_recursive(
        platform_sparse_repodata,
        vec![package_name.clone()],
        None,
    )
    .into_diagnostic()?;

    // Solve for environment
    // Construct a solver task that we can start solving.
    let task = rattler_solve::SolverTask {
        specs: vec![package_matchspec],
        available_packages: &available_packages,

        virtual_packages: rattler_virtual_packages::VirtualPackage::current()
            .into_diagnostic()?
            .iter()
            .cloned()
            .map(Into::into)
            .collect(),

        locked_packages: vec![],
        pinned_packages: vec![],
    };

    // Solve it
    let records = resolvo::Solver.solve(task).into_diagnostic()?;

    // Create the binary environment prefix where we install or update the package
    let BinEnvDir(bin_prefix) = BinEnvDir::create(&package_name).await?;
    let prefix = Prefix::new(bin_prefix);
    let prefix_records = prefix.find_installed_packages(None).await?;

    // Create the transaction that we need
    let transaction = Transaction::from_current_and_desired(
        prefix_records.clone(),
        records.iter().cloned(),
        Platform::current(),
    )
    .into_diagnostic()?;

    let has_transactions = !transaction.operations.is_empty();

    // Execute the transaction if there is work to do
    if has_transactions {
        // Execute the operations that are returned by the solver.
        await_in_progress(
            "creating virtual environment",
            execute_transaction(
                &transaction,
                &prefix_records,
                prefix.root().to_path_buf(),
                config::get_cache_dir()?,
                authenticated_client,
            ),
        )
        .await?;
    }

    // Find the installed package in the environment
    let prefix_package = find_designated_package(&prefix, &package_name).await?;

    // Determine the shell to use for the invocation script
    let shell: ShellEnum = if cfg!(windows) {
        rattler_shell::shell::CmdExe.into()
    } else {
        rattler_shell::shell::Bash.into()
    };

    // Construct the reusable activation script for the shell and generate an invocation script
    // for each executable added by the package to the environment.
    let activation_script = create_activation_script(&prefix, shell.clone())?;
    let bin_dir = BinDir::create().await?;
    let script_mapping =
        find_and_map_executable_scripts(&prefix, &prefix_package, &bin_dir).await?;
    create_executable_scripts(&script_mapping, &prefix, &shell, activation_script).await?;

    let scripts: Vec<_> = script_mapping
        .into_iter()
        .map(
            |BinScriptMapping {
                 global_binary_path: path,
                 ..
             }| path,
        )
        .collect();

    // Check if the bin path is on the path
    if scripts.is_empty() {
        let channel = channel_name_from_prefix(&prefix_package, channel_config);
        miette::bail!(
            "could not find an executable entrypoint in package {} {} {} from {}, are you sure it exists?",
            console::style(prefix_package.repodata_record.package_record.name.as_source()).bold(),
            console::style(prefix_package.repodata_record.package_record.version).bold(),
            console::style(prefix_package.repodata_record.package_record.build).bold(),
            channel,
        );
    }

    Ok((prefix_package, scripts, has_transactions))
}

fn channel_name_from_prefix(
    prefix_package: &PrefixRecord,
    channel_config: &ChannelConfig,
) -> String {
    Channel::from_str(&prefix_package.repodata_record.channel, channel_config)
        .map(|ch| friendly_channel_name(&ch))
        .unwrap_or_else(|_| prefix_package.repodata_record.channel.clone())
}

pub(super) fn package_name(package_matchspec: &MatchSpec) -> miette::Result<PackageName> {
    package_matchspec.name.clone().ok_or_else(|| {
        miette::miette!(
            "could not find package name in MatchSpec {}",
            package_matchspec
        )
    })
}

/// Returns the string to add for all arguments passed to the script
fn get_catch_all_arg(shell: &ShellEnum) -> &str {
    match shell {
        ShellEnum::CmdExe(_) => "%*",
        ShellEnum::PowerShell(_) => "@args",
        _ => "\"$@\"",
    }
}

/// Returns true if the bin folder is available on the PATH.
fn is_bin_folder_on_path() -> bool {
    let bin_path = match bin_dir() {
        Ok(path) => path,
        Err(_) => return false,
    };

    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect_vec())
        .unwrap_or_default()
        .into_iter()
        .contains(&bin_path)
}
