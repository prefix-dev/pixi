use crate::repodata::friendly_channel_name;
use crate::{
    environment::execute_transaction, prefix::Prefix, progress::await_in_progress,
    repodata::fetch_sparse_repodata,
};
use clap::Parser;
use dirs::home_dir;
use itertools::Itertools;
use rattler::install::Transaction;
use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, Platform, PrefixRecord};
use rattler_networking::AuthenticatedClient;
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::Shell,
    shell::ShellEnum,
};
use rattler_solve::{LibsolvRepoData, SolverBackend};
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

const BIN_DIR: &str = ".pixi/bin";
const BIN_ENVS_DIR: &str = ".pixi/envs";

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

struct BinDir(pub PathBuf);

impl BinDir {
    /// Create the Binary Executable directory
    pub async fn create() -> anyhow::Result<Self> {
        let bin_dir = bin_dir()?;
        tokio::fs::create_dir_all(&bin_dir).await?;
        Ok(Self(bin_dir))
    }
}

/// Binaries are installed in ~/.pixi/bin
fn bin_dir() -> anyhow::Result<PathBuf> {
    Ok(home_dir()
        .ok_or_else(|| anyhow::anyhow!("could not find home directory"))?
        .join(BIN_DIR))
}

struct BinEnvDir(pub PathBuf);

impl BinEnvDir {
    /// Create the Binary Environment directory
    pub async fn create(package_name: &str) -> anyhow::Result<Self> {
        let bin_env_dir = bin_env_dir()?.join(package_name);
        tokio::fs::create_dir_all(&bin_env_dir).await?;
        Ok(Self(bin_env_dir))
    }
}

/// Binary environments are installed in ~/.pixi/envs
fn bin_env_dir() -> anyhow::Result<PathBuf> {
    Ok(home_dir()
        .ok_or_else(|| anyhow::anyhow!("could not find home directory"))?
        .join(BIN_ENVS_DIR))
}

/// Find the designated package in the prefix
async fn find_designated_package(
    prefix: &Prefix,
    package_name: &str,
) -> anyhow::Result<PrefixRecord> {
    let prefix_records = prefix.find_installed_packages(None).await?;
    prefix_records
        .into_iter()
        .find(|r| r.repodata_record.package_record.name == package_name)
        .ok_or_else(|| anyhow::anyhow!("could not find {} in prefix", package_name))
}

/// Create the environment activation script
fn create_activation_script(prefix: &Prefix, shell: ShellEnum) -> anyhow::Result<String> {
    let activator = Activator::from_path(prefix.root(), shell, Platform::Osx64)?;
    let result = activator.activation(ActivationVariables {
        conda_prefix: None,
        path: None,
    })?;
    Ok(result.script)
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

/// Create the executable scripts by modifying the activation script
/// to activate the environment and run the executable
async fn create_executable_scripts(
    prefix: &Prefix,
    prefix_package: &PrefixRecord,
    shell: &ShellEnum,
    activation_script: String,
) -> anyhow::Result<Vec<String>> {
    let executables = prefix_package
        .files
        .iter()
        .filter(|relative_path| is_executable(prefix, relative_path));

    let mut scripts = Vec::new();
    let bin_dir = BinDir::create().await?;
    for exec in executables {
        let mut script = activation_script.clone();
        shell
            .run_command(
                &mut script,
                [
                    prefix.root().join(exec).to_string_lossy().as_ref(),
                    get_catch_all_arg(shell),
                ],
            )
            .expect("should never fail");

        let file_name = exec
            .file_stem()
            .ok_or_else(|| anyhow::anyhow!("could not get filename from {}", exec.display()))?;
        let mut executable_script_path = bin_dir.0.join(file_name);

        if cfg!(windows) {
            executable_script_path.set_extension("bat");
        };

        tokio::fs::write(&executable_script_path, script).await?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                executable_script_path,
                std::fs::Permissions::from_mode(0o744),
            )?;
        }

        scripts.push(file_name.to_string_lossy().into_owned());
    }
    Ok(scripts)
}

/// Install a global command
pub async fn execute(args: Args) -> anyhow::Result<()> {
    // Figure out what channels we are using
    let channel_config = ChannelConfig::default();
    let channels = args
        .channel
        .iter()
        .map(|c| Channel::from_str(c, &channel_config))
        .collect::<Result<Vec<Channel>, _>>()?;

    // Find the MatchSpec we want to install
    let package_matchspec = MatchSpec::from_str(&args.package)?;
    let package_name = package_matchspec.name.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "could not find package name in MatchSpec {}",
            package_matchspec
        )
    })?;
    let platform = Platform::current();

    // Fetch sparse repodata
    let platform_sparse_repodata = fetch_sparse_repodata(&channels, &[platform]).await?;

    let available_packages = SparseRepoData::load_records_recursive(
        platform_sparse_repodata.iter(),
        vec![package_name.clone()],
    )?;

    // Solve for environment
    // Construct a solver task that we can start solving.
    let task = rattler_solve::SolverTask {
        specs: vec![package_matchspec],
        available_packages: available_packages
            .iter()
            .map(|records| LibsolvRepoData::from_records(records)),

        virtual_packages: rattler_virtual_packages::VirtualPackage::current()?
            .iter()
            .cloned()
            .map(Into::into)
            .collect(),

        locked_packages: vec![],
        pinned_packages: vec![],
    };

    // Solve it
    let records = rattler_solve::LibsolvBackend.solve(task)?;

    // Create the binary environment prefix where we install or update the package
    let bin_prefix = BinEnvDir::create(&package_name).await?;
    let prefix = Prefix::new(bin_prefix.0)?;
    let prefix_records = prefix.find_installed_packages(None).await?;

    // Create the transaction that we need
    let transaction =
        Transaction::from_current_and_desired(prefix_records, records.iter().cloned(), platform)?;

    // Execute the transaction if there is work to do
    if !transaction.operations.is_empty() {
        // Execute the operations that are returned by the solver.
        await_in_progress(
            "creating virtual environment",
            execute_transaction(
                transaction,
                prefix.root().to_path_buf(),
                rattler::default_cache_dir()?,
                AuthenticatedClient::default(),
            ),
        )
        .await?;
    }

    // Find the installed package in the environment
    let prefix_package = find_designated_package(&prefix, &package_name).await?;
    let channel = Channel::from_str(&prefix_package.repodata_record.channel, &channel_config)
        .map(|ch| friendly_channel_name(&ch))
        .unwrap_or_else(|_| prefix_package.repodata_record.channel.clone());

    // Determine the shell to use for the invocation script
    let shell: ShellEnum = if cfg!(windows) {
        rattler_shell::shell::CmdExe.into()
    } else {
        rattler_shell::shell::Bash.into()
    };

    // Construct the reusable activation script for the shell and generate an invocation script
    // for each executable added by the package to the environment.
    let activation_script = create_activation_script(&prefix, shell.clone())?;
    let script_names =
        create_executable_scripts(&prefix, &prefix_package, &shell, activation_script).await?;

    // Check if the bin path is on the path
    if script_names.is_empty() {
        anyhow::bail!(
            "could not find an executable entrypoint in package {} {} {} from {}, are you sure it exists?",
            console::style(prefix_package.repodata_record.package_record.name).bold(),
            console::style(prefix_package.repodata_record.package_record.version).bold(),
            console::style(prefix_package.repodata_record.package_record.build).bold(),
            channel,
        );
    } else {
        let whitespace = console::Emoji("  ", "").to_string();
        eprintln!(
            "{}Installed package {} {} {} from {}",
            console::style(console::Emoji("✔ ", "")).green(),
            console::style(prefix_package.repodata_record.package_record.name).bold(),
            console::style(prefix_package.repodata_record.package_record.version).bold(),
            console::style(prefix_package.repodata_record.package_record.build).bold(),
            channel,
        );

        let script_names = script_names
            .into_iter()
            .join(&format!("\n{whitespace} -  "));

        if is_bin_folder_on_path() {
            eprintln!(
                "{whitespace}These apps are now globally available:\n{whitespace} -  {script_names}",
            )
        } else {
            let bin_dir = format!("~/{BIN_DIR}");
            eprintln!("{whitespace}These apps have been added to {}\n{whitespace} -  {script_names}\n\n{} To use them, make sure to add {} to your PATH",
                      console::style(&bin_dir).bold(),
                      console::style("!").yellow().bold(),
                      console::style(&bin_dir).bold()
            )
        }
    }

    Ok(())
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
