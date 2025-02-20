use std::sync::{Arc, LazyLock};

use crate::build::{BuildContext, SourceCheckoutReporter};
use crate::environment::PythonStatus;
use crate::lock_file::IoConcurrencyLimit;
use crate::prefix::{Prefix, PrefixError};
use crate::workspace::grouped_environment::{GroupedEnvironment, GroupedEnvironmentName};
use crate::workspace::HasWorkspaceRef;
use futures::{stream, StreamExt, TryFutureExt, TryStreamExt};
use indicatif::ProgressBar;
use itertools::{Either, Itertools};
use miette::IntoDiagnostic;
use pixi_manifest::FeaturesExt;
use pixi_progress::{await_in_progress, global_multi_progress};
use pixi_record::PixiRecord;
use rattler::install::{DefaultProgressFormatter, IndicatifReporter, Installer};
use rattler::package_cache::PackageCache;
use rattler_conda_types::{
    ChannelUrl, GenericVirtualPackage, Platform, PrefixRecord, RepoDataRecord,
};
use reqwest_middleware::ClientWithMiddleware;
use tokio::sync::Semaphore;

use async_once_cell::OnceCell as AsyncOnceCell;
use uv_configuration::RAYON_INITIALIZE;

use super::conda_metadata::{create_history_file, create_prefix_location_file};
use super::reporters::CondaBuildProgress;
use super::try_increase_rlimit_to_sensible;

/// A struct that contains the result of updating a conda prefix.

#[derive(Clone)]
pub struct CondaPrefixUpdated {
    /// The name of the group that was updated.
    pub group: GroupedEnvironmentName,
    /// The prefix that was updated.
    pub prefix: Prefix,
    /// Any change to the python interpreter.
    pub python_status: Box<PythonStatus>,
}

/// A task that updates the prefix for a given environment.
pub struct CondaPrefixUpdaterInner {
    pub channels: Vec<ChannelUrl>,
    pub name: GroupedEnvironmentName,
    pub client: ClientWithMiddleware,
    pub prefix: Prefix,
    pub virtual_packages: Vec<GenericVirtualPackage>,
    pub platform: Platform,
    pub package_cache: PackageCache,
    pub io_concurrency_limit: IoConcurrencyLimit,
    pub build_context: BuildContext,

    /// A flag that indicates if the prefix was created.
    created: AsyncOnceCell<CondaPrefixUpdated>,
}

impl CondaPrefixUpdaterInner {
    /// Creates a new prefix task.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        channels: Vec<ChannelUrl>,
        name: GroupedEnvironmentName,
        client: ClientWithMiddleware,
        prefix: Prefix,
        virtual_packages: Vec<GenericVirtualPackage>,
        platform: Platform,
        package_cache: PackageCache,
        io_concurrency_limit: IoConcurrencyLimit,
        build_context: BuildContext,
    ) -> Self {
        Self {
            channels,
            name,
            client,
            prefix,
            virtual_packages,
            platform,
            package_cache,
            io_concurrency_limit,
            build_context,
            created: AsyncOnceCell::new(),
        }
    }
}

/// A builder for creating a new conda prefix updater.
pub struct CondaPrefixUpdaterBuilder<'a> {
    group: GroupedEnvironment<'a>,
    platform: Platform,
    package_cache: PackageCache,
    io_concurrency_limit: IoConcurrencyLimit,
    build_context: BuildContext,
}

impl<'a> CondaPrefixUpdaterBuilder<'a> {
    /// Creates a new builder.
    pub fn new(
        group: GroupedEnvironment<'a>,
        platform: Platform,
        package_cache: PackageCache,
        io_concurrency_limit: IoConcurrencyLimit,
        build_context: BuildContext,
    ) -> Self {
        Self {
            group,
            platform,
            package_cache,
            io_concurrency_limit,
            build_context,
        }
    }

    /// Builds the conda prefix updater by extracting the necessary information from the group.
    pub fn build(self) -> miette::Result<CondaPrefixUpdater> {
        let channels = self
            .group
            .channel_urls(&self.group.workspace().channel_config())
            .into_diagnostic()?;
        let name = self.group.name();
        let prefix = self.group.prefix();
        let virtual_packages = self.group.virtual_packages(self.platform);
        let client = self.group.workspace().authenticated_client()?.clone();

        Ok(CondaPrefixUpdater::new(
            channels,
            name,
            client,
            prefix,
            virtual_packages,
            self.platform,
            self.package_cache,
            self.io_concurrency_limit,
            self.build_context,
        ))
    }
}

#[derive(Clone)]
/// A task that updates the prefix for a given environment.
pub struct CondaPrefixUpdater {
    inner: Arc<CondaPrefixUpdaterInner>,
}

impl CondaPrefixUpdater {
    /// Creates a new prefix task.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        channels: Vec<ChannelUrl>,
        name: GroupedEnvironmentName,
        client: ClientWithMiddleware,
        prefix: Prefix,
        virtual_packages: Vec<GenericVirtualPackage>,
        platform: Platform,
        package_cache: PackageCache,
        io_concurrency_limit: IoConcurrencyLimit,
        build_context: BuildContext,
    ) -> Self {
        let inner = CondaPrefixUpdaterInner::new(
            channels,
            name,
            client,
            prefix,
            virtual_packages,
            platform,
            package_cache,
            io_concurrency_limit,
            build_context,
        );

        Self {
            inner: Arc::new(inner),
        }
    }

    /// Updates the prefix for the given environment.
    pub async fn update(
        &self,
        pixi_records: Vec<PixiRecord>,
    ) -> miette::Result<&CondaPrefixUpdated> {
        self.inner
            .created
            .get_or_try_init(async {
                tracing::debug!("updating prefix for '{}'", self.inner.name.fancy_display());

                let channels = self.inner.channels.clone();

                // Spawn a task to determine the currently installed packages.
                let prefix_clone = self.inner.prefix.clone();
                let installed_packages_future =
                    tokio::task::spawn_blocking(move || prefix_clone.find_installed_packages())
                        .unwrap_or_else(|e| match e.try_into_panic() {
                            Ok(panic) => std::panic::resume_unwind(panic),
                            Err(_e) => Err(PrefixError::JoinError),
                        });

                // Wait until the conda records are available and until the installed packages
                // for this prefix are available.
                let installed_packages = installed_packages_future.await?;

                let has_existing_packages = !installed_packages.is_empty();
                let group_name = self.inner.name.clone();
                let client = self.inner.client.clone();

                let python_status = update_prefix_conda(
                    &self.inner.prefix,
                    self.inner.package_cache.clone(),
                    client,
                    installed_packages,
                    pixi_records,
                    self.inner.virtual_packages.clone(),
                    channels,
                    self.inner.platform,
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
                    self.inner.io_concurrency_limit.clone().into(),
                    self.inner.build_context.clone(),
                )
                .await?;

                Ok(CondaPrefixUpdated {
                    group: group_name,
                    prefix: self.inner.prefix.clone(),
                    python_status: Box::new(python_status),
                })
            })
            .await
    }

    pub(crate) fn name(&self) -> &GroupedEnvironmentName {
        &self.inner.name
    }
}

/// Updates the environment to contain the packages from the specified lock-file
#[allow(clippy::too_many_arguments)]
pub async fn update_prefix_conda(
    prefix: &Prefix,
    package_cache: PackageCache,
    authenticated_client: ClientWithMiddleware,
    installed_packages: Vec<PrefixRecord>,
    pixi_records: Vec<PixiRecord>,
    virtual_packages: Vec<GenericVirtualPackage>,
    channels: Vec<ChannelUrl>,
    host_platform: Platform,
    progress_bar_message: &str,
    progress_bar_prefix: &str,
    io_concurrency_limit: Arc<Semaphore>,
    build_context: BuildContext,
) -> miette::Result<PythonStatus> {
    // Try to increase the rlimit to a sensible value for installation.
    try_increase_rlimit_to_sensible();

    // HACK: The `Installer` created below, as well as some code in building
    // packages from source will utilize rayon for parallelism. By using rayon
    // it will implicitly initialize a global thread pool. However, uv
    // has a mechanism to initialize rayon itself, which will crash if the global
    // thread pool was already initialized. To prevent this, we force uv the
    // initialize the rayon global thread pool, this ensures that any rayon code
    // that is run will use the same thread pool.
    //
    // One downside of this approach is that perhaps it turns out that we won't need
    // the thread pool at all (because no changes needed to happen for instance).
    // There is a little bit of overhead when that happens, but I don't see another
    // way around that.
    LazyLock::force(&RAYON_INITIALIZE);

    let (mut repodata_records, source_records): (Vec<_>, Vec<_>) = pixi_records
        .into_iter()
        .partition_map(|record| match record {
            PixiRecord::Binary(record) => Either::Left(record),
            PixiRecord::Source(record) => Either::Right(record),
        });

    let mut progress_reporter = None;
    let mut source_reporter = None;
    let source_pb = global_multi_progress().add(ProgressBar::hidden());

    let source_records_length = source_records.len();
    // Build conda packages out of the source records
    let mut processed_source_packages = stream::iter(source_records)
        .map(Ok)
        .and_then(|record| {
            // If we don't have a progress reporter, create one
            // This is done so that the progress bars are not displayed if there are no
            // source packages
            let progress_reporter = progress_reporter
                .get_or_insert_with(|| {
                    Arc::new(CondaBuildProgress::new(source_records_length as u64))
                })
                .clone();

            let source_reporter = source_reporter
                .get_or_insert_with(|| {
                    Arc::new(SourceCheckoutReporter::new(
                        source_pb.clone(),
                        global_multi_progress(),
                    ))
                })
                .clone();
            let build_id = progress_reporter.associate(record.package_record.name.as_source());
            let build_context = &build_context;
            let channels = &channels;
            let virtual_packages = &virtual_packages;
            async move {
                build_context
                    .build_source_record(
                        &record,
                        channels,
                        host_platform,
                        virtual_packages.clone(),
                        virtual_packages.clone(),
                        progress_reporter.clone(),
                        Some(source_reporter),
                        build_id,
                    )
                    .await
            }
        })
        .try_collect::<Vec<RepoDataRecord>>()
        .await?;

    // Extend the repodata records with the built packages
    repodata_records.append(&mut processed_source_packages);

    // Execute the operations that are returned by the solver.
    let result = await_in_progress(
        format!("{progress_bar_prefix}{progress_bar_message}",),
        |pb| async {
            Installer::new()
                .with_download_client(authenticated_client)
                .with_io_concurrency_semaphore(io_concurrency_limit)
                .with_execute_link_scripts(false)
                .with_installed_packages(installed_packages)
                .with_target_platform(host_platform)
                .with_package_cache(package_cache)
                .with_reporter(
                    IndicatifReporter::builder()
                        .with_multi_progress(global_multi_progress())
                        .with_placement(rattler::install::Placement::After(pb))
                        .with_formatter(
                            DefaultProgressFormatter::default()
                                .with_prefix(format!("{progress_bar_prefix}  ")),
                        )
                        .clear_when_done(true)
                        .finish(),
                )
                .install(prefix.root(), repodata_records)
                .await
                .into_diagnostic()
        },
    )
    .await?;

    // Mark the location of the prefix
    create_prefix_location_file(prefix.root())?;
    create_history_file(prefix.root())?;

    // Determine if the python version changed.
    Ok(PythonStatus::from_transaction(&result.transaction))
}
