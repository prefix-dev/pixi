use futures::TryFutureExt;
use miette::IntoDiagnostic;
use pixi_manifest::FeaturesExt;
use pixi_record::PixiRecord;
use rattler::package_cache::PackageCache;
use rattler_conda_types::Platform;

use crate::{
    build::BuildContext,
    environment::{self, PythonStatus},
    lock_file::IoConcurrencyLimit,
    prefix::Prefix,
    workspace::{
        grouped_environment::{GroupedEnvironment, GroupedEnvironmentName},
        HasWorkspaceRef,
    },
};

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
}

impl<'a> CondaPrefixUpdater<'a> {
    /// Creates a new prefix task.
    pub fn new(
        group: GroupedEnvironment<'a>,
        platform: Platform,
        package_cache: PackageCache,
        io_concurrency_limit: IoConcurrencyLimit,
        build_context: BuildContext,
    ) -> Self {
        Self {
            group,
            package_cache,
            io_concurrency_limit,
            build_context,
            platform,
        }
    }

    /// Updates the prefix for the given environment.
    pub(crate) async fn update(
        &self,
        pixi_records: Vec<PixiRecord>,
    ) -> miette::Result<CondaPrefixUpdated> {
        tracing::debug!(
            "updating prefix for '{}'",
            self.group.name().fancy_display()
        );

        let channels = self
            .group
            .channel_urls(&self.group.workspace().channel_config())
            .into_diagnostic()?;

        // Spawn a task to determine the currently installed packages.
        let prefix_clone = self.group.prefix().clone();
        let installed_packages_future =
            tokio::task::spawn_blocking(move || prefix_clone.find_installed_packages())
                .unwrap_or_else(|e| match e.try_into_panic() {
                    Ok(panic) => std::panic::resume_unwind(panic),
                    Err(_err) => Err(miette::miette!("the operation was cancelled")),
                });

        // Wait until the conda records are available and until the installed packages
        // for this prefix are available.
        let installed_packages = installed_packages_future.await?;

        let has_existing_packages = !installed_packages.is_empty();
        let group_name = self.group.name().clone();
        let client = self.group.workspace().authenticated_client()?.clone();
        let prefix = self.group.prefix();

        let python_status = environment::update_prefix_conda(
            &prefix,
            self.package_cache.clone(),
            client,
            installed_packages,
            pixi_records,
            self.group.virtual_packages(self.platform),
            channels,
            self.platform,
            &format!(
                "{} conda prefix '{}'",
                if has_existing_packages {
                    "updating"
                } else {
                    "creating"
                },
                group_name.fancy_display()
            ),
            "  ",
            self.io_concurrency_limit.clone().into(),
            self.build_context.clone(),
        )
        .await?;

        Ok(CondaPrefixUpdated {
            group: group_name,
            prefix,
            python_status: Box::new(python_status),
        })
    }
}
