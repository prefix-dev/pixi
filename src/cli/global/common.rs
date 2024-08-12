use std::path::PathBuf;

use miette::{Context, IntoDiagnostic};
use pixi_progress::{await_in_progress, wrap_in_progress};
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, PackageName, Platform, PrefixRecord,
    RepoDataRecord,
};
use rattler_repodata_gateway::Gateway;
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::VirtualPackage;

use crate::{prefix::Prefix, repodata};
use pixi_config::home_path;

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

    /// Get the Binary Executable directory, erroring if it doesn't already
    /// exist.
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
    /// Construct the path to the env directory for the binary package
    /// `package_name`.
    fn package_bin_env_dir(package_name: &PackageName) -> miette::Result<PathBuf> {
        Ok(bin_env_dir()
            .ok_or(miette::miette!(
                "could not find global binary environment directory"
            ))?
            .join(package_name.as_normalized()))
    }

    /// Get the Binary Environment directory, erroring if it doesn't already
    /// exist.
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

/// Solve package records from [`Gateway`] for the given package MatchSpec
///
/// # Returns
///
/// The package records (with dependencies records) for the given package
/// MatchSpec
pub async fn solve_package_records<AsChannel, ChannelIter>(
    gateway: &Gateway,
    channels: ChannelIter,
    specs: Vec<MatchSpec>,
) -> miette::Result<Vec<RepoDataRecord>>
where
    AsChannel: Into<Channel>,
    ChannelIter: IntoIterator<Item = AsChannel>,
{
    // Get the repodata for the specs
    let repodata = await_in_progress("fetching repodata for environment", |_| async {
        gateway
            .query(
                channels,
                [Platform::current(), Platform::NoArch],
                specs.clone(),
            )
            .recursive(true)
            .execute()
            .await
    })
    .await
    .into_diagnostic()
    .context("failed to get repodata")?;

    // Determine virtual packages of the current platform
    let virtual_packages = VirtualPackage::current()
        .into_diagnostic()
        .context("failed to determine virtual packages")?
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .collect();

    // Solve the environment
    let solved_records = wrap_in_progress("solving environment", move || {
        Solver.solve(SolverTask {
            specs,
            virtual_packages,
            ..SolverTask::from_iter(&repodata)
        })
    })
    .into_diagnostic()
    .context("failed to solve environment")?;

    Ok(solved_records)
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
