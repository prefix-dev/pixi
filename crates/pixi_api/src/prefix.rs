use itertools::Itertools;
use miette::{Context, Diagnostic, IntoDiagnostic};
use pixi_utils::{is_binary_folder, strip_executable_extension};
use rattler_conda_types::{PackageName, Platform, PrefixRecord};
use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::ShellEnum,
};
use std::sync::LazyLock;
use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
};
use thiserror::Error;
use uv_configuration::RAYON_INITIALIZE;

#[derive(Error, Debug, Diagnostic)]
pub enum PrefixError {
    #[error("failed to collect prefix records from '{1}'")]
    #[diagnostic(help("try `pixi clean` to reset the environment and run the command again"))]
    PrefixRecordCollectionError(#[source] std::io::Error, PathBuf),

    #[error("failed to find the designated package '{0}' in the prefix: '{1}'")]
    DesignatedPackageNotFound(String, PathBuf),

    #[error("executing prefix related task failed")]
    #[diagnostic(help("try running the command again, or `pixi clean` to reset the environment"))]
    JoinError,
}

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

    /// Scans the `conda-meta` directory of an environment and returns all the
    /// [`PrefixRecord`]s found in there.
    pub fn find_installed_packages(&self) -> Result<Vec<PrefixRecord>, PrefixError> {
        // Initialize rayon explicitly to avoid implicit initialization.
        LazyLock::force(&RAYON_INITIALIZE);

        PrefixRecord::collect_from_prefix(&self.root)
            .map_err(|err| PrefixError::PrefixRecordCollectionError(err, self.root.clone()))
    }

    /// Processes prefix records (that you can get by using `find_installed_packages`)
    /// to filter and collect executable files.
    /// Processes prefix records (that you can get by using
    /// `find_installed_packages`) to filter and collect executable files.
    pub fn find_executables(&self, prefix_packages: &[PrefixRecord]) -> Vec<Executable> {
        let executables = prefix_packages
            .iter()
            .flat_map(|record| {
                record
                    .files
                    .iter()
                    .filter(|relative_path| self.is_executable(relative_path))
                    .filter_map(|path| {
                        path.iter().next_back().and_then(OsStr::to_str).map(|name| {
                            Executable::new(
                                strip_executable_extension(name.to_string()),
                                path.clone(),
                            )
                        })
                    })
            })
            .collect();
        tracing::debug!(
            "In packages: {}, found executables: {:?} ",
            prefix_packages
                .iter()
                .map(|rec| rec.repodata_record.package_record.name.as_normalized())
                .join(", "),
            executables
        );
        executables
    }

    /// Checks if the given relative path points to an executable file.
    pub(crate) fn is_executable(&self, relative_path: &Path) -> bool {
        let parent_folder = match relative_path.parent() {
            Some(dir) => dir,
            None => return false,
        };

        if !is_binary_folder(parent_folder) {
            return false;
        }

        // Check if the file is executable
        let absolute_path = self.root().join(relative_path);
        is_executable::is_executable(absolute_path)
    }

    /// Find the designated package in the given [`Prefix`]
    ///
    /// # Returns
    ///
    /// The PrefixRecord of the designated package
    pub async fn find_designated_package(
        &self,
        package_name: &PackageName,
    ) -> Result<PrefixRecord, PrefixError> {
        let prefix_records = self.find_installed_packages()?;
        prefix_records
            .into_iter()
            .find(|r| r.repodata_record.package_record.name == *package_name)
            .ok_or(PrefixError::DesignatedPackageNotFound(
                package_name.as_normalized().to_string(),
                self.root.clone(),
            ))
    }
}

#[derive(Debug, Clone)]
pub struct Executable {
    pub name: String,
    pub path: PathBuf,
}

impl Executable {
    pub fn new(name: String, path: PathBuf) -> Self {
        Self { name, path }
    }
}
