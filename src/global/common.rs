use super::{EnvironmentName, ExposedName};
use fs_err as fs;
use fs_err::tokio as tokio_fs;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pixi_config::home_path;
use rattler_conda_types::{PackageRecord, PrefixRecord};
use std::ffi::OsStr;
use std::fmt;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// Global binaries directory, default to `$HOME/.pixi/bin`
#[derive(Debug, Clone)]
pub struct BinDir(PathBuf);

impl BinDir {
    /// Create the binary executable directory from path
    #[cfg(test)]
    pub fn new(root: PathBuf) -> miette::Result<Self> {
        let path = root.join("bin");
        std::fs::create_dir_all(&path).into_diagnostic()?;
        Ok(Self(path))
    }

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
        // Add .bat to the windows executable
        let exposed_name = if cfg!(windows) {
            // Not using `.set_extension()` because it will break the `.` in the name for cases like `python3.9.1`
            format!("{}.bat", exposed_name)
        } else {
            exposed_name.to_string()
        };
        self.path().join(exposed_name)
    }
}

/// Global environments directory, default to `$HOME/.pixi/envs`
#[derive(Debug, Clone)]
pub struct EnvRoot(PathBuf);

impl EnvRoot {
    /// Create the environment root directory
    #[cfg(test)]
    pub fn new(root: PathBuf) -> miette::Result<Self> {
        let path = root.join("envs");
        std::fs::create_dir_all(&path).into_diagnostic()?;
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
}

/// A global environment directory
pub(crate) struct EnvDir {
    pub(crate) path: PathBuf,
}

impl EnvDir {
    // Create EnvDir from path
    pub(crate) fn from_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Create a global environment directory based on passed global environment root
    pub(crate) async fn from_env_root(
        env_root: EnvRoot,
        environment_name: &EnvironmentName,
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

/// Finds the package record from the `conda-meta` directory.
pub(crate) async fn find_package_records(conda_meta: &Path) -> miette::Result<Vec<PrefixRecord>> {
    let mut read_dir = tokio_fs::read_dir(conda_meta).await.into_diagnostic()?;
    let mut records = Vec::new();

    while let Some(entry) = read_dir.next_entry().await.into_diagnostic()? {
        let path = entry.path();
        // Check if the entry is a file and has a .json extension
        if path.is_file() && path.extension().and_then(OsStr::to_str) == Some("json") {
            let prefix_record = PrefixRecord::from_path(&path)
                .into_diagnostic()
                .wrap_err_with(|| format!("Could not parse json from {}", path.display()))?;

            records.push(prefix_record);
        }
    }

    if records.is_empty() {
        miette::bail!("No package records found in {}", conda_meta.display());
    }

    Ok(records)
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub(crate) enum StateChange {
    AddedExposed(String, EnvironmentName),
    RemovedExposed(String, EnvironmentName),
    UpdatedExposed(String, EnvironmentName),
    AddedPackage(PackageRecord, EnvironmentName),
    AddedEnvironment(EnvironmentName),
    RemovedEnvironment(EnvironmentName),
}

impl StateChange {
    fn env(&self) -> &EnvironmentName {
        match self {
            StateChange::AddedExposed(_, env)
            | StateChange::RemovedExposed(_, env)
            | StateChange::UpdatedExposed(_, env)
            | StateChange::AddedPackage(_, env)
            | StateChange::AddedEnvironment(env)
            | StateChange::RemovedEnvironment(env) => env,
        }
    }
}

impl fmt::Display for StateChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateChange::AddedExposed(exposed, env) => {
                write!(
                    f,
                    "{}Added exposed '{exposed}' for environment '{env}'.",
                    console::style(console::Emoji("✔ ", "")).green(),
                )
            }
            StateChange::RemovedExposed(exposed, env) => {
                write!(
                    f,
                    "{}Removed exposed '{exposed}' for environment '{env}'.",
                    console::style(console::Emoji("✔ ", "")).green(),
                )
            }
            StateChange::UpdatedExposed(exposed, env) => {
                write!(
                    f,
                    "{}Updated exposed '{exposed}' for environment '{env}'.",
                    console::style(console::Emoji("✔ ", "")).green(),
                )
            }
            StateChange::AddedPackage(pkg, env) => write!(
                f,
                "{}Added package '{}={}' to environment '{env}'.",
                console::style(console::Emoji("✔ ", "")).green(),
                pkg.name.as_normalized(),
                pkg.version,
            ),
            StateChange::AddedEnvironment(env) => write!(
                f,
                "{}Added environment '{env}'.",
                console::style(console::Emoji("✔ ", "")).green()
            ),
            StateChange::RemovedEnvironment(env) => write!(
                f,
                "{}Removed environment '{env}'.",
                console::style(console::Emoji("✔ ", "")).green()
            ),
        }
    }
}

#[must_use]
#[derive(Debug, Default)]
pub(crate) struct StateChanges {
    changes: Vec<StateChange>,
    has_updated: bool,
}

impl StateChanges {
    pub(crate) fn has_changed(&self) -> bool {
        self.has_updated || !self.changes.is_empty()
    }

    pub(crate) fn set_has_updated(&mut self, has_updated: bool) {
        self.has_updated = has_updated;
    }

    pub(crate) fn push_change(&mut self, change: StateChange) {
        self.changes.push(change);
    }

    pub(crate) fn push_changes(&mut self, changes: impl IntoIterator<Item = StateChange>) {
        self.changes.extend(changes);
    }

    #[cfg(test)]
    pub fn changes(self) -> Vec<StateChange> {
        self.changes
    }

    /// Remove changes that cancel each other out
    fn prune(&mut self) {
        // Remove changes if the environment is removed afterwards
        let mut pruned_changes: Vec<StateChange> = Vec::new();
        for change in &self.changes {
            match change {
                StateChange::RemovedEnvironment(env) => {
                    pruned_changes.retain(|change| match change {
                        StateChange::RemovedEnvironment(_) => true,
                        c => c.env() != env,
                    });
                }
                _ => {
                    pruned_changes.push(change.clone());
                }
            }
        }
        self.changes = pruned_changes;
    }

    pub(crate) fn report(mut self) {
        self.prune();
        let report = self.changes.iter().join("\n");
        eprintln!("{report}");
    }
}

impl std::ops::BitOrAssign for StateChanges {
    fn bitor_assign(&mut self, rhs: Self) {
        self.changes.extend(rhs.changes);
        self.has_updated |= rhs.has_updated;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::str::FromStr;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_create() {
        // Create a temporary directory
        let temp_dir = tempdir().unwrap();

        // Set the env root to the temporary directory
        let env_root = EnvRoot::new(temp_dir.path().to_owned()).unwrap();

        // Define a test environment name
        let environment_name = &EnvironmentName::from_str("test-env").unwrap();

        // Create a new binary env dir
        let bin_env_dir = EnvDir::from_env_root(env_root, environment_name)
            .await
            .unwrap();

        // Verify that the directory was created
        assert!(bin_env_dir.path().exists());
        assert!(bin_env_dir.path().is_dir());
    }

    #[tokio::test]
    async fn test_find_package_record() {
        // Get meta file from test data folder relative to the current file
        let dummy_conda_meta_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("global")
            .join("test_data")
            .join("conda-meta");
        // Find the package record
        let records = find_package_records(&dummy_conda_meta_path).await.unwrap();

        // Verify that the package record was found
        assert!(records
            .iter()
            .any(|rec| rec.repodata_record.package_record.name.as_normalized() == "python"));
    }

    #[rstest]
    #[case("python3.9.1")]
    #[case("python3.9")]
    #[case("python3")]
    #[case("python")]
    fn test_executable_script_path(#[case] exposed_name: &str) {
        let path = PathBuf::from("/home/user/.pixi/bin");
        let bin_dir = BinDir(path.clone());
        let exposed_name = ExposedName::from_str(exposed_name).unwrap();
        let executable_script_path = bin_dir.executable_script_path(&exposed_name);

        if cfg!(windows) {
            let expected = format!("{}.bat", exposed_name);
            assert_eq!(executable_script_path, path.join(expected));
        } else {
            assert_eq!(executable_script_path, path.join(exposed_name.to_string()));
        }
    }
}
