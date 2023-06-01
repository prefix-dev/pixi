use crate::environment::execute_transaction;
use crate::prefix::Prefix;
use crate::progress::await_in_progress;
use crate::repodata::{fetch_sparse_repodata, friendly_channel_name};
use clap::Parser;
use dirs::home_dir;
use rattler::install::Transaction;
use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, Platform, PrefixRecord};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_shell::activation::{ActivationVariables, Activator};
use rattler_solve::{LibsolvRepoData, SolverBackend};
use reqwest::Client;
use std::ops::Add;
use std::path::PathBuf;
use std::str::FromStr;

/// Runs command in project.
#[derive(Parser, Debug)]
#[clap(trailing_var_arg = true)]
pub struct Args {
    /// Package to install
    package: String,

    /// Channel to install from
    #[clap(short, long, default_values = ["conda-forge"])]
    channels: Vec<String>,
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

/// Binaries are installed in ~/.pax/bin
fn bin_dir() -> anyhow::Result<PathBuf> {
    Ok(home_dir()
        .ok_or_else(|| anyhow::anyhow!("could not find home directory"))?
        .join(".pax/bin"))
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

/// Binary environments are installed in ~/.pax/envs
fn bin_env_dir() -> anyhow::Result<PathBuf> {
    Ok(home_dir()
        .ok_or_else(|| anyhow::anyhow!("could not find home directory"))?
        .join(".pax/envs"))
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
fn create_activation_script(prefix: &Prefix) -> anyhow::Result<String> {
    let shell_type = rattler_shell::shell::ShellEnum::detect_from_environment()
        .ok_or_else(|| anyhow::anyhow!("Could not detect shell type"))?;
    let activator = Activator::from_path(prefix.root(), shell_type, Platform::Osx64)?;
    let result = activator.activation(ActivationVariables {
        conda_prefix: None,
        path: Some(vec![
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
            PathBuf::from("/usr/sbin"),
            PathBuf::from("/sbin"),
            PathBuf::from("/usr/local/bin"),
        ]),
    })?;
    Ok(result.script)
}

/// Create the executable scripts by modifying the activation script
/// to activate the environment and run the executable
async fn create_executable_scripts(
    prefix: &Prefix,
    prefix_package: &PrefixRecord,
    activation_script: String,
) -> anyhow::Result<()> {
    let executables = prefix_package
        .files
        .iter()
        .filter(|f| f.starts_with("bin/") && is_executable::is_executable(prefix.root().join(f)));

    let bin_dir = BinDir::create().await?;
    for exec in executables {
        let script = activation_script.clone().add(&format!(
            "\n ${{CONDA_PREFIX}}/{}",
            exec.to_str()
                .expect("could not convert path to string")
        ));
        let filename =
            bin_dir.0.join(exec.file_name().ok_or_else(|| {
                anyhow::anyhow!("could not get filename from {}", exec.display())
            })?);
        tokio::fs::write(&filename, script).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(filename, std::fs::Permissions::from_mode(0o744))?;
        }
    }
    Ok(())
}

/// Install a global command
pub async fn execute(args: Args) -> anyhow::Result<()> {
    // Figure out what channels we are using
    let channels = args
        .channels
        .iter()
        .map(|c| Channel::from_str(c, &ChannelConfig::default()))
        .collect::<Result<Vec<Channel>, _>>()?;

    // Find the matchspec we want to install
    let package_matchspec = MatchSpec::from_str(&args.package)?;
    let package_name = package_matchspec.name.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "could not find package name in matchspec {}",
            package_matchspec
        )
    })?;
    let platform = Platform::current();

    println!(
        "Installing: {}, from {}",
        package_matchspec,
        channels
            .iter()
            .map(friendly_channel_name)
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Fetch sparse repodata
    let platform_sparse_repodata = fetch_sparse_repodata(&channels, &vec![platform]).await?;

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

        locked_packages: vec![],
        pinned_packages: vec![],
        virtual_packages: vec![],
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
            "installing environment",
            execute_transaction(
                transaction,
                prefix.root().to_path_buf(),
                rattler::default_cache_dir()?,
                Client::default(),
            ),
        )
        .await?;
    }

    let prefix_package = find_designated_package(&prefix, &package_name).await?;
    let activation_script = create_activation_script(&prefix)?;
    create_executable_scripts(&prefix, &prefix_package, activation_script).await?;

    Ok(())
}
