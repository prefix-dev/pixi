use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use futures::{stream::FuturesUnordered, StreamExt};
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{Platform, PrefixRecord};
use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::ShellEnum,
};
use tokio::task::JoinHandle;

/// Points to a directory that serves as a Conda prefix.
#[derive(Debug, Clone)]
pub struct Prefix {
    root: PathBuf,
}

impl Prefix {
    /// Constructs a new instance.
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        let root = path.into();
        Self { root }
    }

    /// Returns the root directory of the prefix
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Runs the activation scripts of the prefix and returns the environment
    /// variables that were modified as part of this process.
    pub async fn run_activation(&self) -> miette::Result<HashMap<String, String>> {
        let activator =
            Activator::from_path(self.root(), ShellEnum::default(), Platform::current())
                .into_diagnostic()
                .context("failed to constructor environment activator")?;

        activator
            .run_activation(ActivationVariables::from_env().unwrap_or_default(), None)
            .into_diagnostic()
            .context("failed to run activation")
    }

    /// Scans the `conda-meta` directory of an environment and returns all the
    /// [`PrefixRecord`]s found in there.
    pub async fn find_installed_packages(
        &self,
        concurrency_limit: Option<usize>,
    ) -> miette::Result<Vec<PrefixRecord>> {
        let concurrency_limit = concurrency_limit.unwrap_or(100);
        let mut meta_futures = FuturesUnordered::<JoinHandle<miette::Result<PrefixRecord>>>::new();
        let mut result = Vec::new();
        for entry in std::fs::read_dir(self.root.join("conda-meta"))
            .into_iter()
            .flatten()
        {
            let entry = entry.into_diagnostic()?;
            let path = entry.path();
            if !path.is_file() || path.extension() != Some("json".as_ref()) {
                continue;
            }

            // If there are too many pending entries, wait for one to be finished
            if meta_futures.len() >= concurrency_limit {
                match meta_futures
                    .next()
                    .await
                    .expect("we know there are pending futures")
                {
                    Ok(record) => result.push(record?),
                    Err(e) => {
                        if let Ok(panic) = e.try_into_panic() {
                            std::panic::resume_unwind(panic);
                        }
                        // The future was cancelled, we can simply return what we have.
                        return Ok(result);
                    }
                }
            }

            // Spawn loading on another thread
            let future = tokio::task::spawn_blocking(move || {
                PrefixRecord::from_path(&path)
                    .into_diagnostic()
                    .with_context(move || format!("failed to parse '{}'", path.display()))
            });
            meta_futures.push(future);
        }

        while let Some(record) = meta_futures.next().await {
            match record {
                Ok(record) => result.push(record?),
                Err(e) => {
                    if let Ok(panic) = e.try_into_panic() {
                        std::panic::resume_unwind(panic);
                    }
                    // The future was cancelled, we can simply return what we have.
                    return Ok(result);
                }
            }
        }

        Ok(result)
    }

    /// Processes prefix records (that you can get by using `find_installed_packages`)
    /// to filter and collect executable files.
    pub fn find_executables(&self, prefix_packages: &[PrefixRecord]) -> Vec<(String, PathBuf)> {
        prefix_packages
            .iter()
            .flat_map(|record| {
                record
                    .files
                    .iter()
                    .filter(|relative_path| self.is_executable(relative_path))
                    .filter_map(|path| {
                        path.file_stem()
                            .and_then(OsStr::to_str)
                            .map(|name| (name.to_string(), path.clone()))
                    })
            })
            .collect()
    }

    /// Checks if the given relative path points to an executable file.
    pub(crate) fn is_executable(&self, relative_path: &Path) -> bool {
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
        let absolute_path = self.root().join(relative_path);
        is_executable::is_executable(absolute_path)
    }
}
