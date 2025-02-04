use super::utils::IoConcurrencyLimit;
use crate::build::BuildContext;
use crate::environment::{self, PythonStatus};
use crate::prefix::Prefix;
use crate::project::grouped_environment::{GroupedEnvironment, GroupedEnvironmentName};
use crate::project::HasProjectRef;
use futures::TryFutureExt;
use miette::IntoDiagnostic;
use pixi_manifest::FeaturesExt;
use pixi_record::PixiRecord;
use rattler::package_cache::PackageCache;
use rattler_conda_types::Platform;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// A struct that contains the result of updating a conda prefix.
pub struct CondaPrefixUpdated {
    /// The name of the group that was updated.
    pub group: GroupedEnvironmentName,
    /// The prefix that was updated.
    pub prefix: Prefix,
    /// Any change to the python interpreter.
    pub python_status: Box<PythonStatus>,
}

#[derive(Clone)]
/// A task that updates the prefix for a given environment.
pub struct CondaPrefixUpdater<'a> {
    pub group: GroupedEnvironment<'a>,
    pub platform: Platform,
    pub package_cache: PackageCache,
    pub io_concurrency_limit: IoConcurrencyLimit,
    pub build_context: BuildContext,
    pub no_install: bool,
}

impl<'a> CondaPrefixUpdater<'a> {
    /// Creates a new prefix task.
    pub fn new(
        group: GroupedEnvironment<'a>,
        platform: Platform,
        package_cache: PackageCache,
        io_concurrency_limit: IoConcurrencyLimit,
        build_context: BuildContext,
        no_install: bool,
    ) -> Self {
        Self {
            group,
            package_cache,
            io_concurrency_limit,
            build_context,
            no_install,
            platform,
        }
    }

    /// Updates the prefix for the given environment.
    pub(crate) async fn update(
        &self,
        pixi_records: Vec<PixiRecord>,
    ) -> miette::Result<CondaPrefixUpdated> {
        if self.no_install {
            miette::bail!("Cannot install prefix when `--no-install` is set");
        }
        tracing::debug!(
            "updating prefix for '{}'",
            self.group.name().fancy_display()
        );

        // Get the required group names
        let group_name = self.group.name().clone();
        let prefix = self.group.prefix();
        let client = self.group.project().authenticated_client().clone();

        let channels = self
            .group
            .channel_urls(&self.group.project().channel_config())
            .into_diagnostic()?;

        // Spawn a task to determine the currently installed packages.
        let prefix_clone = prefix.clone();
        let installed_packages_future =
            tokio::task::spawn_blocking(move || prefix_clone.find_installed_packages())
                .unwrap_or_else(|e| match e.try_into_panic() {
                    Ok(panic) => std::panic::resume_unwind(panic),
                    Err(_err) => Err(miette::miette!("the operation was cancelled")),
                });

        // Wait until the conda records are available and until the installed packages
        // for this prefix are available.
        let installed_packages = installed_packages_future.await?;
        let build_virtual_packages = self.group.virtual_packages(self.platform);

        let concurrency_limit: Arc<Semaphore> = self.io_concurrency_limit.clone().into();
        let package_cache = self.package_cache.clone();
        let build_context = self.build_context.clone();
        let has_existing_packages = !installed_packages.is_empty();
        let python_status = environment::update_prefix_conda(
            &prefix,
            package_cache,
            client,
            installed_packages,
            pixi_records,
            build_virtual_packages,
            channels,
            self.platform,
            &format!(
                "{} python environment to solve pypi packages for '{}'",
                if has_existing_packages {
                    "updating"
                } else {
                    "creating"
                },
                group_name.fancy_display()
            ),
            "  ",
            concurrency_limit,
            build_context,
        )
        .await?;

        Ok(CondaPrefixUpdated {
            group: group_name,
            prefix,
            python_status: Box::new(python_status),
        })
    }
}
