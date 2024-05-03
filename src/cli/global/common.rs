use std::path::PathBuf;

use indexmap::IndexMap;
use miette::IntoDiagnostic;
use rattler_conda_types::{
    Channel, ChannelConfig, MatchSpec, PackageName, Platform, PrefixRecord, RepoDataRecord,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{resolvo, ChannelPriority, SolverImpl, SolverTask};
use reqwest_middleware::ClientWithMiddleware;

use crate::{
    config::{home_path, Config},
    prefix::Prefix,
    repodata,
    utils::reqwest::build_reqwest_clients,
};

/// Global binaries directory, default to `$HOME/.pixi/bin`
pub struct BinDir(pub PathBuf);

impl BinDir {
    /// Create the Binary Executable directory
    pub async fn create() -> miette::Result<Self> {
        let bin_dir = bin_dir().ok_or(miette::miette!(
            "could not determine global binary executable directory"
        ))?;
        tokio::fs::create_dir_all(&bin_dir)
            .await
            .into_diagnostic()?;
        Ok(Self(bin_dir))
    }

    /// Get the Binary Executable directory, erroring if it doesn't already exist.
    pub async fn from_existing() -> miette::Result<Self> {
        let bin_dir = bin_dir().ok_or(miette::miette!(
            "could not find global binary executable directory"
        ))?;
        if tokio::fs::try_exists(&bin_dir).await.into_diagnostic()? {
            Ok(Self(bin_dir))
        } else {
            Err(miette::miette!(
                "binary executable directory does not exist"
            ))
        }
    }
}

/// Global binary environments directory, default to `$HOME/.pixi/envs`
pub struct BinEnvDir(pub PathBuf);

impl BinEnvDir {
    /// Construct the path to the env directory for the binary package `package_name`.
    fn package_bin_env_dir(package_name: &PackageName) -> miette::Result<PathBuf> {
        Ok(bin_env_dir()
            .ok_or(miette::miette!(
                "could not find global binary environment directory"
            ))?
            .join(package_name.as_normalized()))
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

/// Global binaries directory, default to `$HOME/.pixi/bin`
///
/// # Returns
///
/// The global binaries directory
pub fn bin_dir() -> Option<PathBuf> {
    home_path().map(|path| path.join("bin"))
}

/// Global binary environments directory, default to `$HOME/.pixi/envs`
///
/// # Returns
///
/// The global binary environments directory
pub fn bin_env_dir() -> Option<PathBuf> {
    home_path().map(|path| path.join("envs"))
}

/// Returns the package name from a MatchSpec
///
/// # Returns
///
/// The package name from the given MatchSpec
pub(super) fn package_name(package_matchspec: &MatchSpec) -> miette::Result<PackageName> {
    package_matchspec.name.clone().ok_or_else(|| {
        miette::miette!(
            "could not find package name in MatchSpec {}",
            package_matchspec
        )
    })
}

/// Get the friendly channel name of a [`PrefixRecord`]
///
/// # Returns
///
/// The friendly channel name of the given prefix record
pub(super) fn channel_name_from_prefix(
    prefix_package: &PrefixRecord,
    channel_config: &ChannelConfig,
) -> String {
    Channel::from_str(&prefix_package.repodata_record.channel, channel_config)
        .map(|ch| repodata::friendly_channel_name(&ch))
        .unwrap_or_else(|_| prefix_package.repodata_record.channel.clone())
}

/// Load package records from [`SparseRepoData`] for the given package MatchSpec
///
/// # Returns
///
/// The package records (with dependencies records) for the given package MatchSpec
pub fn load_package_records(
    package_matchspec: MatchSpec,
    sparse_repodata: &IndexMap<(Channel, Platform), SparseRepoData>,
) -> miette::Result<Vec<RepoDataRecord>> {
    let package_name = package_name(&package_matchspec)?;
    let available_packages =
        SparseRepoData::load_records_recursive(sparse_repodata.values(), vec![package_name], None)
            .into_diagnostic()?;
    let virtual_packages = rattler_virtual_packages::VirtualPackage::current()
        .into_diagnostic()?
        .iter()
        .cloned()
        .map(Into::into)
        .collect();

    // Solve for environment
    // Construct a solver task that we can start solving.
    let task = SolverTask {
        specs: vec![package_matchspec],
        available_packages: &available_packages,
        virtual_packages,
        locked_packages: vec![],
        pinned_packages: vec![],
        timeout: None,
        channel_priority: ChannelPriority::Strict,
    };

    // Solve it
    let records = resolvo::Solver.solve(task).into_diagnostic()?;

    Ok(records)
}

/// Get networking Client and fetch [`SparseRepoData`] for the given channels and
/// current platform using the client
///
/// # Returns
///
/// The network client and the fetched sparse repodata
pub(super) async fn get_client_and_sparse_repodata(
    channels: impl IntoIterator<Item = &'_ Channel>,
    platform: Platform,
    config: &Config,
) -> miette::Result<(
    ClientWithMiddleware,
    IndexMap<(Channel, Platform), SparseRepoData>,
)> {
    let authenticated_client = build_reqwest_clients(Some(config)).1;
    let platform_sparse_repodata =
        repodata::fetch_sparse_repodata(channels, [platform], &authenticated_client, Some(config))
            .await?;
    Ok((authenticated_client, platform_sparse_repodata))
}

/// Find the globally installed package with the given [`PackageName`]
///
/// # Returns
///
/// The PrefixRecord of the installed package
pub(super) async fn find_installed_package(
    package_name: &PackageName,
) -> miette::Result<PrefixRecord> {
    let BinEnvDir(bin_prefix) = BinEnvDir::from_existing(package_name).await.or_else(|_| {
        miette::bail!(
            "Package {} is not globally installed",
            package_name.as_source()
        )
    })?;
    let prefix = Prefix::new(bin_prefix);
    find_designated_package(&prefix, package_name).await
}

/// Find the designated package in the given [`Prefix`]
///
/// # Returns
///
/// The PrefixRecord of the designated package
pub async fn find_designated_package(
    prefix: &Prefix,
    package_name: &PackageName,
) -> miette::Result<PrefixRecord> {
    let prefix_records = prefix.find_installed_packages(None).await?;
    prefix_records
        .into_iter()
        .find(|r| r.repodata_record.package_record.name == *package_name)
        .ok_or_else(|| miette::miette!("could not find {} in prefix", package_name.as_source()))
}
