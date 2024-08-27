use std::path::{Path, PathBuf};

use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pixi_progress::{await_in_progress, global_multi_progress};
use rattler::{
    install::{DefaultProgressFormatter, IndicatifReporter, Installer},
    package_cache::PackageCache,
};
use rattler_conda_types::{
    Channel, ChannelConfig, PackageName, Platform, PrefixRecord, RepoDataRecord,
};
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::ShellEnum,
};
use reqwest_middleware::ClientWithMiddleware;

use crate::{
    cli::project::environment, prefix::Prefix, repodata, rlimit::try_increase_rlimit_to_sensible,
};
use pixi_config::home_path;

use super::EnvironmentName;

/// Global binaries directory, default to `$HOME/.pixi/bin`
pub struct BinDir(PathBuf);

impl BinDir {
    /// Create the Binary Executable directory
    pub async fn from_env() -> miette::Result<Self> {
        let bin_dir = home_path()
            .map(|path| path.join("bin"))
            .ok_or(miette::miette!(
                "could not determine global binary executable directory"
            ))?;
        tokio::fs::create_dir_all(&bin_dir)
            .await
            .into_diagnostic()?;
        Ok(Self(bin_dir))
    }

    /// Asynchronously retrieves all files in the Binary Executable directory.
    ///
    /// This function reads the directory specified by `self.0` and collects all
    /// file paths into a vector. It returns a `miette::Result` containing the
    /// vector of file paths or an error if the directory cannot be read.
    pub(crate) async fn files(&self) -> miette::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.0)
            .await
            .into_diagnostic()
            .wrap_err_with(|| format!("Could not read {}", &self.0.display()))?;

        while let Some(entry) = entries.next_entry().await.into_diagnostic()? {
            let path = entry.path();
            if path.is_file() {
                files.push(path);
            }
        }

        Ok(files)
    }

    /// Returns the path to the binary directory
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Returns the path to the executable script for the given exposed name.
    ///
    /// This function constructs the path to the executable script by joining the
    /// `bin_dir` with the provided `exposed_name`. If the target platform is
    /// Windows, it sets the file extension to `.bat`.
    pub(crate) fn executable_script_path(&self, exposed_name: &str) -> PathBuf {
        let mut executable_script_path = self.0.join(exposed_name);
        if cfg!(windows) {
            executable_script_path.set_extension("bat");
        }
        executable_script_path
    }

    pub(crate) async fn print_executables_available(
        &self,
        executables: Vec<PathBuf>,
    ) -> miette::Result<()> {
        let whitespace = console::Emoji("  ", "").to_string();
        let executable = executables
            .into_iter()
            .map(|path| {
                path.strip_prefix(self.path())
                    .expect("script paths were constructed by joining onto BinDir")
                    .to_string_lossy()
                    .to_string()
            })
            .join(&format!("\n{whitespace} -  "));

        if self.is_on_path() {
            eprintln!(
                "{whitespace}These executables are now globally available:\n{whitespace} -  {executable}",
            )
        } else {
            eprintln!("{whitespace}These executables have been added to {}\n{whitespace} -  {executable}\n\n{} To use them, make sure to add {} to your PATH",
                      console::style(&self.path().display()).bold(),
                      console::style("!").yellow().bold(),
                      console::style(&self.path().display()).bold()
            )
        }

        Ok(())
    }

    /// Returns true if the bin folder is available on the PATH.
    fn is_on_path(&self) -> bool {
        let Some(path_content) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&path_content).contains(&self.path().to_owned())
    }
}

#[derive(Debug, Clone)]
pub struct EnvRoot(PathBuf);

impl EnvRoot {
    pub async fn new(path: PathBuf) -> miette::Result<Self> {
        tokio::fs::create_dir_all(&path).await.into_diagnostic()?;
        Ok(Self(path))
    }

    pub async fn from_env() -> miette::Result<Self> {
        let path = home_path()
            .map(|path| path.join("envs"))
            .ok_or_else(|| miette::miette!("Could not get home path"))?;
        tokio::fs::create_dir_all(&path).await.into_diagnostic()?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Delete environments that are not listed
    pub(crate) async fn prune(
        &self,
        environments: impl IntoIterator<Item = EnvironmentName>,
    ) -> miette::Result<()> {
        let env_set: ahash::HashSet<EnvironmentName> = environments.into_iter().collect();
        let mut entries = tokio::fs::read_dir(&self.path())
            .await
            .into_diagnostic()
            .wrap_err_with(|| format!("Could not read directory {}", self.path().display()))?;

        while let Some(entry) = entries.next_entry().await.into_diagnostic()? {
            let path = entry.path();
            if path.is_dir() {
                let Some(Ok(dir_name)) = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.parse())
                else {
                    continue;
                };
                if !env_set.contains(&dir_name) {
                    tokio::fs::remove_dir_all(&path)
                        .await
                        .into_diagnostic()
                        .wrap_err_with(|| {
                            format!("Could not remove directory {}", path.display())
                        })?;
                }
            }
        }

        Ok(())
    }
}

/// Global binary environments directory
pub(crate) struct EnvDir {
    root: EnvRoot,
    path: PathBuf,
}

impl EnvDir {
    /// Create the Binary Environment directory
    pub(crate) async fn new(
        root: EnvRoot,
        environment_name: EnvironmentName,
    ) -> miette::Result<Self> {
        let path = root.path().join(environment_name.as_str());
        tokio::fs::create_dir_all(&path).await.into_diagnostic()?;

        Ok(Self { root, path })
    }

    /// Construct the path to the env directory for the environment
    /// `environment_name`.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// Get the friendly channel name of a [`PrefixRecord`]
///
/// # Returns
///
/// The friendly channel name of the given prefix record
pub(crate) fn channel_name_from_prefix(
    prefix_package: &PrefixRecord,
    channel_config: &ChannelConfig,
) -> String {
    Channel::from_str(&prefix_package.repodata_record.channel, channel_config)
        .map(|ch| repodata::friendly_channel_name(&ch))
        .unwrap_or_else(|_| prefix_package.repodata_record.channel.clone())
}

/// Find the designated package in the given [`Prefix`]
///
/// # Returns
///
/// The PrefixRecord of the designated package
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_create() {
        // Create a temporary directory
        let temp_dir = tempdir().unwrap();

        // Set the env root to the temporary directory
        let env_root = EnvRoot::new(temp_dir.path().to_owned()).await.unwrap();

        // Define a test environment name
        let environment_name = "test-env".parse().unwrap();

        // Create a new binary env dir
        let bin_env_dir = EnvDir::new(env_root, environment_name).await.unwrap();

        // Verify that the directory was created
        assert!(bin_env_dir.path().exists());
        assert!(bin_env_dir.path().is_dir());
    }

    #[tokio::test]
    async fn test_prune() {
        // Create a temporary directory
        let temp_dir = tempdir().unwrap();

        // Set the env root to the temporary directory
        let env_root = EnvRoot::new(temp_dir.path().to_owned()).await.unwrap();

        // Create some directories in the temporary directory
        let envs = ["env1", "env2", "env3"];
        for env in &envs {
            EnvDir::new(env_root.clone(), env.parse().unwrap())
                .await
                .unwrap();
        }

        // Call the prune method with a list of environments to keep
        env_root
            .prune(["env1".parse().unwrap(), "env3".parse().unwrap()])
            .await
            .unwrap();

        // Verify that only the specified directories remain
        let remaining_dirs = std::fs::read_dir(env_root.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().into_string().unwrap())
            .sorted()
            .collect_vec();

        assert_eq!(remaining_dirs, vec!["env1", "env3"]);
    }
}
