use super::{EnvironmentName, ExposedName};
use fs_err as fs;
use fs_err::tokio as tokio_fs;
use miette::IntoDiagnostic;
use pixi_config::home_path;
use pixi_consts::consts;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

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
        tokio_fs::create_dir_all(&bin_dir).await.into_diagnostic()?;
        Ok(Self(bin_dir))
    }

    /// Asynchronously retrieves all files in the binary executable directory.
    ///
    /// This function reads the directory specified by `self.0` and collects all
    /// file paths into a vector. It returns a `miette::Result` containing the
    /// vector of file paths or an error if the directory cannot be read.
    pub(crate) async fn files(&self) -> miette::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let mut entries = tokio_fs::read_dir(&self.0).await.into_diagnostic()?;

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
    pub(crate) fn executable_script_path(&self, exposed_name: &ExposedName) -> PathBuf {
        let mut executable_script_path = self.0.join(exposed_name.to_string());
        if cfg!(windows) {
            executable_script_path.set_extension("bat");
        }
        executable_script_path
    }
}

/// Global environoments directory, default to `$HOME/.pixi/envs`
#[derive(Debug, Clone)]
pub struct EnvRoot(PathBuf);

impl EnvRoot {
    /// Create the environment root directory
    #[cfg(test)]
    pub async fn new(path: PathBuf) -> miette::Result<Self> {
        tokio_fs::create_dir_all(&path).await.into_diagnostic()?;
        Ok(Self(path))
    }

    /// Create the environment root directory from environment variables
    pub(crate) async fn from_env() -> miette::Result<Self> {
        let path = home_path()
            .map(|path| path.join("envs"))
            .ok_or_else(|| miette::miette!("Could not get home path"))?;
        tokio_fs::create_dir_all(&path).await.into_diagnostic()?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Get all directories in the env root
    pub(crate) async fn directories(&self) -> miette::Result<Vec<PathBuf>> {
        let mut directories = Vec::new();
        let mut entries = tokio_fs::read_dir(&self.path()).await.into_diagnostic()?;

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
    ) -> miette::Result<Vec<PathBuf>> {
        let env_set: ahash::HashSet<EnvironmentName> = environments_to_keep.into_iter().collect();

        let mut pruned = Vec::new();
        for env_path in self.directories().await? {
            let Some(Ok(env_name)) = env_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.parse())
            else {
                continue;
            };

            if !env_set.contains(&env_name) {
                // Test if the environment directory is a conda environment
                if let Ok(true) = env_path.join(consts::CONDA_META_DIR).try_exists() {
                    // Remove the conda environment
                    tokio_fs::remove_dir_all(&env_path)
                        .await
                        .into_diagnostic()?;
                    pruned.push(env_path);
                    eprintln!(
                        "{} Remove environment '{env_name}'",
                        console::style(console::Emoji("✔", " ")).green()
                    );
                }
            }
        }

        Ok(pruned)
    }
}

/// A global environment directory
pub(crate) struct EnvDir {
    pub(crate) path: PathBuf,
}

impl EnvDir {
    /// Create a global environment directory based on passed global environment root
    pub(crate) async fn from_env_root(
        env_root: EnvRoot,
        environment_name: EnvironmentName,
    ) -> miette::Result<Self> {
        let path = env_root.path().join(environment_name.as_str());
        tokio_fs::create_dir_all(&path).await.into_diagnostic()?;

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
    let mut file = fs::File::open(file_path.as_ref()).into_diagnostic()?;
    let mut buffer = [0; 1024];
    let bytes_read = file.read(&mut buffer).into_diagnostic()?;

    Ok(buffer[..bytes_read].contains(&0))
}

/// Checks if given path points to a text file by calling `is_binary`.
/// If that returns `false`, then it is a text file and vice-versa.
pub(crate) fn is_text(file_path: impl AsRef<Path>) -> miette::Result<bool> {
    Ok(!is_binary(file_path)?)
}

/// Strips known Windows executable extensions from a file name.
pub(crate) fn strip_windows_executable_extension(file_name: String) -> String {
    let file_name = file_name.to_lowercase();
    // Attempt to retrieve the PATHEXT environment variable
    let extensions_list: Vec<String> = if let Ok(pathext) = std::env::var("PATHEXT") {
        pathext.split(';').map(|s| s.to_lowercase()).collect()
    } else {
        // Fallback to a default list if PATHEXT is not set
        tracing::debug!("Could not find 'PATHEXT' variable, using a default list");
        [
            ".COM", ".EXE", ".BAT", ".CMD", ".VBS", ".VBE", ".JS", ".JSE", ".WSF", ".WSH", ".MSC",
            ".CPL",
        ]
        .iter()
        .map(|s| s.to_lowercase())
        .collect()
    };

    // Attempt to strip any known Windows executable extension
    extensions_list
        .iter()
        .find_map(|ext| file_name.strip_suffix(ext))
        .map(|f| f.to_string())
        .unwrap_or(file_name)
}

/// Strips known Unix executable extensions from a file name.
pub(crate) fn strip_unix_executable_extension(file_name: String) -> String {
    let file_name = file_name.to_lowercase();

    // Define a list of common Unix executable extensions
    let extensions_list: Vec<&str> = vec![
        ".sh", ".bash", ".zsh", ".csh", ".tcsh", ".ksh", ".fish", ".py", ".pl", ".rb", ".lua",
        ".php", ".tcl", ".awk", ".sed",
    ];

    // Attempt to strip any known Unix executable extension
    extensions_list
        .iter()
        .find_map(|&ext| file_name.strip_suffix(ext))
        .map(|f| f.to_string())
        .unwrap_or(file_name)
}

/// Strips known executable extensions from a file name based on the target operating system.
///
/// This function acts as a wrapper that calls either `strip_windows_executable_extension`
/// or `strip_unix_executable_extension` depending on the target OS.
pub(crate) fn executable_from_path(path: &Path) -> String {
    let file_name = path
        .iter()
        .last()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .to_string();

    if cfg!(target_family = "windows") {
        strip_windows_executable_extension(file_name)
    } else {
        strip_unix_executable_extension(file_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err::tokio as tokio_fs;
    use itertools::Itertools;
    use rstest::rstest;
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
        let envs = ["env1", "env2", "env3", "non-conda-env-dir"];
        for env in &envs {
            EnvDir::from_env_root(env_root.clone(), env.parse().unwrap())
                .await
                .unwrap();
        }
        // Add conda meta data to env2 to make sure it's seen as a conda environment
        tokio_fs::create_dir_all(env_root.path().join("env2").join(consts::CONDA_META_DIR))
            .await
            .unwrap();

        // Call the prune method with a list of environments to keep (env1 and env3) but not env4
        env_root
            .prune(["env1".parse().unwrap(), "env3".parse().unwrap()])
            .await
            .unwrap();

        // Verify that only the specified directories remain
        let remaining_dirs = fs::read_dir(env_root.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().into_string().unwrap())
            .sorted()
            .collect_vec();

        assert_eq!(remaining_dirs, vec!["env1", "env3", "non-conda-env-dir"]);
    }

    #[rstest]
    #[case::python312_linux("python3.12", "python3.12")]
    #[case::python3_linux("python3", "python3")]
    #[case::python_linux("python", "python")]
    #[case::python3121_linux("python3.12.1", "python3.12.1")]
    #[case::bash_script("bash.sh", "bash")]
    #[case::zsh59("zsh-5.9", "zsh-5.9")]
    #[case::python_312config("python3.12-config", "python3.12-config")]
    #[case::python3_config("python3-config", "python3-config")]
    #[case::x2to3("2to3", "2to3")]
    #[case::x2to3312("2to3-3.12", "2to3-3.12")]
    fn test_strip_executable_unix(#[case] path: &str, #[case] expected: &str) {
        let path = Path::new(path);
        let result = strip_unix_executable_extension(path.to_string_lossy().to_string());
        assert_eq!(result, expected);
    }

    #[rstest]
    #[case::python_windows("python.exe", "python")]
    #[case::python3_windows("python3.exe", "python3")]
    #[case::python312_windows("python3.12.exe", "python3.12")]
    #[case::bash("bash", "bash")]
    #[case::zsh59("zsh-5.9", "zsh-5.9")]
    #[case::python_312config("python3.12-config", "python3.12-config")]
    #[case::python3_config("python3-config", "python3-config")]
    #[case::x2to3("2to3", "2to3")]
    #[case::x2to3312("2to3-3.12", "2to3-3.12")]
    fn test_strip_executable_windows(#[case] path: &str, #[case] expected: &str) {
        let path = Path::new(path);
        let result = strip_windows_executable_extension(path.to_string_lossy().to_string());
        assert_eq!(result, expected);
    }
}
