use futures::stream::FuturesUnordered;
use futures::StreamExt;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{Platform, PrefixRecord};
use rattler_shell::activation::{ActivationVariables, Activator};
use rattler_shell::shell::ShellEnum;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::task::JoinHandle;

/// Points to a directory that serves as a Conda prefix.
#[derive(Debug, Clone)]
pub struct Prefix {
    root: PathBuf,
}

impl Prefix {
    /// Constructs a new instance.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let root = path.into();
        Self { root }
    }

    /// Returns the root directory of the prefix
    pub fn root(&self) -> &Path {
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

    /// Scans the `conda-meta` directory of an environment and returns all the [`PrefixRecord`]s found
    /// in there.
    pub async fn find_installed_packages(
        &self,
        concurrency_limit: Option<usize>,
    ) -> miette::Result<Vec<PrefixRecord>> {
        let concurrency_limit = concurrency_limit.unwrap_or(100);
        let mut meta_futures =
            FuturesUnordered::<JoinHandle<Result<PrefixRecord, std::io::Error>>>::new();
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
                    Ok(record) => result.push(record.into_diagnostic()?),
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
            let future = tokio::task::spawn_blocking(move || PrefixRecord::from_path(path));
            meta_futures.push(future);
        }

        while let Some(record) = meta_futures.next().await {
            match record {
                Ok(record) => result.push(record.into_diagnostic()?),
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
}
