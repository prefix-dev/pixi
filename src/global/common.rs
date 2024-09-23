use std::{
    io::Read,
    path::{Path, PathBuf},
};

use itertools::Itertools;
use miette::{Context, IntoDiagnostic};

use pixi_config::home_path;

use super::{EnvironmentName, ExposedKey};

/// Global binaries directory, default to `$HOME/.pixi/bin`
#[derive(Debug, Clone)]
pub struct BinDir(PathBuf);

impl BinDir {
    /// Create the binary executable directory from environment variables
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

    /// Asynchronously retrieves all files in the binary executable directory.
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
    pub(crate) fn executable_script_path(&self, exposed_name: &ExposedKey) -> PathBuf {
        let mut executable_script_path = self.0.join(exposed_name.to_string());
        if cfg!(windows) {
            executable_script_path.set_extension("bat");
        }
        executable_script_path
    }

    pub async fn print_executables_available(
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

/// Global environoments directory, default to `$HOME/.pixi/envs`
#[derive(Debug, Clone)]
pub struct EnvRoot(PathBuf);

impl EnvRoot {
    /// Create the environment root directory
    #[cfg(test)]
    pub async fn new(path: PathBuf) -> miette::Result<Self> {
        tokio::fs::create_dir_all(&path)
            .await
            .into_diagnostic()
            .wrap_err_with(|| format!("Couldn't create directory {}", path.display()))?;
        Ok(Self(path))
    }

    /// Create the environment root directory from environment variables
    pub(crate) async fn from_env() -> miette::Result<Self> {
        let path = home_path()
            .map(|path| path.join("envs"))
            .ok_or_else(|| miette::miette!("Could not get home path"))?;
        tokio::fs::create_dir_all(&path)
            .await
            .into_diagnostic()
            .wrap_err_with(|| format!("Couldn't create directory {}", path.display()))?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Get all directories in the env root
    pub(crate) async fn directories(&self) -> miette::Result<Vec<PathBuf>> {
        let mut directories = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.path())
            .await
            .into_diagnostic()
            .wrap_err_with(|| format!("Could not read directory {}", self.path().display()))?;

        while let Some(entry) = entries.next_entry().await.into_diagnostic()? {
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
            }
        }

        Ok(directories)
    }

    /// Delete environments that are not listed
    pub(crate) async fn prune(
        &self,
        environments_to_keep: impl IntoIterator<Item = EnvironmentName>,
    ) -> miette::Result<()> {
        let env_set: ahash::HashSet<EnvironmentName> = environments_to_keep.into_iter().collect();

        for env_path in self.directories().await? {
            let Some(Ok(env_name)) = env_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.parse())
            else {
                continue;
            };
            if !env_set.contains(&env_name) {
                tokio::fs::remove_dir_all(&env_path)
                    .await
                    .into_diagnostic()
                    .wrap_err_with(|| {
                        format!("Could not remove directory {}", env_path.display())
                    })?;
                eprintln!(
                    "{} Remove environment '{env_name}'",
                    console::style(console::Emoji("✔", " ")).green()
                );
            }
        }

        Ok(())
    }
}

/// A global environment directory
pub(crate) struct EnvDir {
    pub(crate) path: PathBuf,
}

impl EnvDir {
    /// Create a global environment directory based on passed global environment root
    pub(crate) async fn from_env_root<T: Into<EnvironmentName>>(
        env_root: EnvRoot,
        environment_name: T,
    ) -> miette::Result<Self> {
        let environment_name = environment_name.into();
        let path = env_root.path().join(environment_name.as_str());
        tokio::fs::create_dir_all(&path).await.into_diagnostic()?;

        Ok(Self { path })
    }

    /// Construct the path to the env directory for the environment
    /// `environment_name`.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// Checks if a file is binary by reading the first 1024 bytes and checking for null bytes.
pub(crate) fn is_binary(file_path: impl AsRef<Path>) -> miette::Result<bool> {
    let mut file = std::fs::File::open(&file_path)
        .into_diagnostic()
        .wrap_err_with(|| format!("Could not open {}", &file_path.as_ref().display()))?;
    let mut buffer = [0; 1024];
    let bytes_read = file
        .read(&mut buffer)
        .into_diagnostic()
        .wrap_err_with(|| format!("Could not read {}", &file_path.as_ref().display()))?;

    Ok(buffer[..bytes_read].contains(&0))
}

/// Checks if given path points to a text file by calling `is_binary`.
/// If that returns `false`, then it is a text file and vice-versa.
pub(crate) fn is_text(file_path: impl AsRef<Path>) -> miette::Result<bool> {
    Ok(!is_binary(file_path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[tokio::test]
    async fn test_create() {
        // Create a temporary directory
        let temp_dir = tempdir().unwrap();

        // Set the env root to the temporary directory
        let env_root = EnvRoot::new(temp_dir.path().to_owned()).await.unwrap();

        // Define a test environment name
        let environment_name: EnvironmentName = "test-env".parse().unwrap();

        // Create a new binary env dir
        let bin_env_dir = EnvDir::from_env_root(env_root, environment_name)
            .await
            .unwrap();

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
            EnvDir::from_env_root(env_root.clone(), env.parse::<EnvironmentName>().unwrap())
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
