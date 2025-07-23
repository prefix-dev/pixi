use std::{collections::HashMap, path::Path, str::FromStr, sync::Arc};

use conda_pypi_clobber::PypiCondaClobberRegistry;
use itertools::Itertools;
use miette::{IntoDiagnostic, WrapErr};
use pixi_consts::consts;
use pixi_manifest::{
    EnvironmentName, SystemRequirements,
    pypi::pypi_options::{NoBinary, NoBuild, NoBuildIsolation},
};
use pixi_record::PixiRecord;
use pixi_uv_conversions::{
    BuildIsolation, locked_indexes_to_index_locations, pypi_options_to_build_options,
};
use plan::{InstallPlanner, InstallReason, NeedReinstall, PyPIInstallationPlan};
use pypi_modifiers::{
    Tags,
    pypi_tags::{get_pypi_tags, is_python_record},
};
use rattler_conda_types::Platform;
use rattler_lock::{PypiIndexes, PypiPackageData, PypiPackageEnvironmentData};
use utils::elapsed;
use uv_auth::store_credentials_from_url;
use uv_client::{Connectivity, FlatIndexClient, RegistryClient, RegistryClientBuilder};
use uv_configuration::{BuildOptions, ConfigSettings, Constraints, IndexStrategy, PreviewMode};
use uv_dispatch::{BuildDispatch, SharedState};
use uv_distribution::{DistributionDatabase, RegistryWheelIndex};
use uv_distribution_types::{
    CachedDist, DependencyMetadata, Dist, IndexLocations, IndexUrl, InstalledDist, Name, Resolution,
};
use uv_install_wheel::LinkMode;
use uv_installer::{Preparer, SitePackages, UninstallError};
use uv_python::{Interpreter, PythonEnvironment};
use uv_resolver::FlatIndex;
use uv_types::HashStrategy;
use uv_workspace::WorkspaceCache;

use crate::{
    lock_file::UvResolutionContext,
    prefix::Prefix,
    uv_reporter::{UvReporter, UvReporterOptions},
};

pub(crate) mod conda_pypi_clobber;
pub(crate) mod conversions;
pub(crate) mod install_wheel;
pub(crate) mod plan;
pub(crate) mod utils;

/// Configuration for PyPI environment updates, grouping basic environment settings
pub struct PyPIUpdateConfig<'a> {
    pub environment_name: &'a EnvironmentName,
    pub prefix: &'a Prefix,
    pub platform: Platform,
    pub lock_file_dir: &'a Path,
    pub system_requirements: &'a SystemRequirements,
}

/// Configuration for PyPI build options, grouping all build-related settings
pub struct PyPIBuildConfig<'a> {
    pub non_isolated_packages: &'a NoBuildIsolation,
    pub no_build: &'a NoBuild,
    pub no_binary: &'a NoBinary,
}

/// Configuration for PyPI context, grouping uv and environment settings
pub struct PyPIContextConfig<'a> {
    pub uv_context: &'a UvResolutionContext,
    pub pypi_indexes: Option<&'a PypiIndexes>,
    pub environment_variables: &'a HashMap<String, String>,
}

/// Internal setup data for the uv installer
struct UvInstallerConfig {
    tags: Tags,
    index_locations: IndexLocations,
    build_options: BuildOptions,
    registry_client: Arc<RegistryClient>,
    flat_index: FlatIndex,
    config_settings: ConfigSettings,
    venv: PythonEnvironment,
    build_isolation: BuildIsolation,
}

/// High-level interface for PyPI environment updates that handles all complexity internally
/// This is full of lifetime, because internal uv datastructs require it.
pub struct PyPIEnvironmentUpdater<'a> {
    config: PyPIUpdateConfig<'a>,
    build_config: PyPIBuildConfig<'a>,
    context_config: PyPIContextConfig<'a>,
}

impl<'a> PyPIEnvironmentUpdater<'a> {
    /// Create a new PyPI environment updater with the given configurations
    pub fn new(
        config: PyPIUpdateConfig<'a>,
        build_config: PyPIBuildConfig<'a>,
        context_config: PyPIContextConfig<'a>,
    ) -> Self {
        Self {
            config,
            build_config,
            context_config,
        }
    }

    /// Update PyPI packages in the environment, handling all setup, planning, and execution
    pub async fn update_packages(
        &self,
        pixi_records: &[PixiRecord],
        pypi_records: &[(PypiPackageData, PypiPackageEnvironmentData)],
        python_status: &crate::environment::PythonStatus,
    ) -> miette::Result<()> {
        use crate::environment::{ContinuePyPIPrefixUpdate, on_python_interpreter_change};
        use fancy_display::FancyDisplay;
        use pixi_progress::await_in_progress;

        // Determine global site-packages status
        let python_info =
            match on_python_interpreter_change(python_status, self.config.prefix, pypi_records)
                .await?
            {
                ContinuePyPIPrefixUpdate::Continue(python_info) => python_info,
                ContinuePyPIPrefixUpdate::Skip => return Ok(()),
            };

        // Install and/or remove python packages
        await_in_progress(
            format!(
                "updating pypi packages in '{}'",
                self.config.environment_name.fancy_display()
            ),
            |_| async {
                self.execute_update(pixi_records, pypi_records, &python_info)
                    .await
            },
        )
        .await
    }

    /// Execute the complete PyPI update workflow
    async fn execute_update(
        &self,
        pixi_records: &[PixiRecord],
        pypi_records: &[(PypiPackageData, PypiPackageEnvironmentData)],
        python_info: &rattler::install::PythonInfo,
    ) -> miette::Result<()> {
        // Setup UV environment and configuration
        let setup = self
            .setup_uv_installer_config(pixi_records, &python_info.path)
            .await?;

        // Create installation plan
        let installation_plan = self.create_installation_plan(pypi_records, &setup).await?;

        // Execute the installation plan
        self.execute_installation_plan(&installation_plan, &setup)
            .await
    }

    /// Setup UV environment with all necessary configuration
    async fn setup_uv_installer_config(
        &self,
        pixi_records: &[PixiRecord],
        python_interpreter_path: &Path,
    ) -> miette::Result<UvInstallerConfig> {
        // Determine the current environment markers.
        let python_record = pixi_records
            .iter()
            .find(|r| is_python_record(r))
            .cloned()
            .ok_or_else(|| miette::miette!("could not resolve pypi dependencies because no python interpreter is added to the dependencies of the project.\nMake sure to add a python interpreter to the [dependencies] section of the {manifest}, or run:\n\n\tpixi add python", manifest=consts::WORKSPACE_MANIFEST))?;

        let tags = get_pypi_tags(
            self.config.platform,
            self.config.system_requirements,
            python_record.package_record(),
        )?;

        let index_locations = self
            .context_config
            .pypi_indexes
            .map(|indexes| locked_indexes_to_index_locations(indexes, self.config.lock_file_dir))
            .unwrap_or_else(|| Ok(IndexLocations::default()))
            .into_diagnostic()?;

        let build_options =
            pypi_options_to_build_options(self.build_config.no_build, self.build_config.no_binary)
                .into_diagnostic()?;

        let mut uv_client_builder =
            RegistryClientBuilder::new(self.context_config.uv_context.cache.clone())
                .allow_insecure_host(self.context_config.uv_context.allow_insecure_host.clone())
                .keyring(self.context_config.uv_context.keyring_provider)
                .connectivity(Connectivity::Online)
                .extra_middleware(self.context_config.uv_context.extra_middleware.clone())
                .index_locations(&index_locations);

        for p in &self.context_config.uv_context.proxies {
            uv_client_builder = uv_client_builder.proxy(p.clone())
        }

        let registry_client = Arc::new(uv_client_builder.build());

        // Resolve the flat indexes from `--find-links`.
        let flat_index_client = FlatIndexClient::new(
            registry_client.cached_client(),
            Connectivity::Online,
            &self.context_config.uv_context.cache,
        );

        let flat_index_urls: Vec<&IndexUrl> = index_locations
            .flat_indexes()
            .map(|index| index.url())
            .collect();

        let flat_index_entries = flat_index_client
            .fetch_all(flat_index_urls.into_iter())
            .await
            .into_diagnostic()?;

        let flat_index = FlatIndex::from_entries(
            flat_index_entries,
            Some(&tags),
            &self.context_config.uv_context.hash_strategy,
            &build_options,
        );

        let config_settings = ConfigSettings::default();

        // Setup the interpreter from the conda prefix
        let python_location = self.config.prefix.root().join(python_interpreter_path);
        let interpreter =
            Interpreter::query(&python_location, &self.context_config.uv_context.cache)
                .into_diagnostic()?;

        tracing::debug!(
            "using python interpreter: {} from {}",
            interpreter.key(),
            interpreter.sys_prefix().display()
        );

        // Create a Python environment
        let venv = PythonEnvironment::from_interpreter(interpreter);

        // Determine isolated packages based on input, converting names.
        let build_isolation = self
            .build_config
            .non_isolated_packages
            .clone()
            .try_into()
            .into_diagnostic()?;

        Ok(UvInstallerConfig {
            tags,
            index_locations,
            build_options,
            registry_client,
            flat_index,
            config_settings,
            venv,
            build_isolation,
        })
    }

    /// Create the installation plan by analyzing current state vs requirements
    async fn create_installation_plan(
        &self,
        pypi_records: &[(PypiPackageData, PypiPackageEnvironmentData)],
        setup: &UvInstallerConfig,
    ) -> miette::Result<PyPIInstallationPlan> {
        // Create a map of the required packages
        let required_map: std::collections::HashMap<uv_normalize::PackageName, &PypiPackageData> =
            pypi_records
                .iter()
                .map(|(pkg, _)| {
                    let uv_name = uv_normalize::PackageName::from_str(pkg.name.as_ref())
                        .expect("should be correct");
                    (uv_name, pkg)
                })
                .collect();

        // Find out what packages are already installed
        let site_packages =
            SitePackages::from_environment(&setup.venv).expect("could not create site-packages");

        tracing::debug!(
            "Constructed site-packages with {} packages",
            site_packages.iter().count(),
        );

        // This is used to find wheels that are available from the registry
        let registry_index = RegistryWheelIndex::new(
            &self.context_config.uv_context.cache,
            &setup.tags,
            &setup.index_locations,
            &HashStrategy::None,
            &setup.config_settings,
        );

        // Create installation plan
        let installation_plan = InstallPlanner::new(
            self.context_config.uv_context.cache.clone(),
            self.config.lock_file_dir,
        )
        .plan(&site_packages, registry_index, &required_map)
        .into_diagnostic()
        .context("error while determining PyPI installation plan")?;

        // Show totals
        let total_to_install = installation_plan.local.len() + installation_plan.remote.len();
        let total_required = required_map.len();
        tracing::debug!(
            "{} of {} required packages are considered installed and up-to-date",
            total_required - total_to_install,
            total_required
        );

        Ok(installation_plan)
    }

    /// Execute the installation plan - this is the main installation logic
    async fn execute_installation_plan(
        &self,
        installation_plan: &PyPIInstallationPlan,
        setup: &UvInstallerConfig,
    ) -> miette::Result<()> {
        // Lock before performing operations
        let _lock = setup
            .venv
            .lock()
            .await
            .into_diagnostic()
            .with_context(|| "error locking installation directory")?;

        let start = std::time::Instant::now();
        let PyPIInstallationPlan {
            local,
            remote,
            reinstalls,
            extraneous,
            duplicates,
        } = installation_plan;

        // Nothing to do.
        if remote.is_empty()
            && local.is_empty()
            && reinstalls.is_empty()
            && extraneous.is_empty()
            && duplicates.is_empty()
        {
            tracing::info!(
                "{}",
                format!("Nothing to do - finished in {}", elapsed(start.elapsed()))
            );
            return Ok(());
        }

        // Log installation details for debugging
        self.log_installation_details(local, remote, reinstalls, extraneous, duplicates);

        // Download, build, and unzip any missing distributions.
        let remote_dists = if remote.is_empty() {
            Vec::new()
        } else {
            self.prepare_remote_distributions(remote, setup).await?
        };

        // Remove any duplicate metadata for packages that are now owned by conda
        self.remove_duplicate_metadata(duplicates)
            .into_diagnostic()
            .wrap_err("while removing duplicate metadata")?;

        // Remove any unnecessary packages.
        self.remove_packages(extraneous, reinstalls).await?;

        // Install the resolved distributions.
        let local_dists = local.iter().map(|(d, _)| d.clone());
        let all_dists = remote_dists
            .into_iter()
            .chain(local_dists)
            .collect::<Vec<_>>();

        self.check_and_warn_about_conflicts(&all_dists, reinstalls, setup)
            .await?;

        self.install_distributions(all_dists, setup).await?;
        tracing::info!("{}", format!("finished in {}", elapsed(start.elapsed())));

        Ok(())
    }

    /// Log any interesting installation details.
    fn log_installation_details(
        &self,
        local: &[(CachedDist, InstallReason)],
        remote: &[(Dist, InstallReason)],
        reinstalls: &[(InstalledDist, NeedReinstall)],
        extraneous: &[InstalledDist],
        duplicates: &[InstalledDist],
    ) {
        // Installation and re-installation have needed a lot of debugging in the past
        // That's why we do a bit more extensive logging here
        let mut install_cached = vec![];
        let mut install_stale = vec![];
        let mut install_missing = vec![];

        // Filter out the re-installs, mostly these are less interesting than the actual
        // re-install reasons do show the installs
        for (dist, reason) in local.iter() {
            match reason {
                InstallReason::InstallStaleLocal => install_stale.push(dist.name().to_string()),
                InstallReason::InstallMissing => install_missing.push(dist.name().to_string()),
                InstallReason::InstallCached => install_cached.push(dist.name().to_string()),
                _ => {}
            }
        }

        fn name_and_version(dist: &Dist) -> String {
            format!(
                "{} {}",
                dist.name(),
                dist.version().map(|v| v.to_string()).unwrap_or_default()
            )
        }

        for (dist, reason) in remote.iter() {
            match reason {
                InstallReason::InstallStaleLocal => install_stale.push(name_and_version(dist)),
                InstallReason::InstallMissing => install_missing.push(name_and_version(dist)),
                InstallReason::InstallCached => install_cached.push(name_and_version(dist)),
                _ => {}
            }
        }

        if !install_missing.is_empty() {
            tracing::debug!(
                "*installing* from remote because no version is cached: {}",
                install_missing.iter().join(", ")
            );
        }
        if !install_stale.is_empty() {
            tracing::debug!(
                "*installing* from remote because local version is stale: {}",
                install_stale.iter().join(", ")
            );
        }
        if !install_cached.is_empty() {
            tracing::debug!(
                "*installing* cached version because cache is up-to-date: {}",
                install_cached.iter().join(", ")
            );
        }

        if !reinstalls.is_empty() {
            tracing::debug!(
                "*re-installing* following packages: {}",
                reinstalls
                    .iter()
                    .map(|(d, _)| format!("{} {}", d.name(), d.version()))
                    .join(", ")
            );
            // List all re-install reasons
            for (dist, reason) in reinstalls {
                // Only log the re-install reason if it is not an installer mismatch
                if !matches!(reason, NeedReinstall::InstallerMismatch { .. }) {
                    tracing::debug!(
                        "re-installing '{}' because: '{}'",
                        console::style(dist.name()).blue(),
                        reason
                    );
                }
            }
        }

        if !extraneous.is_empty() {
            // List all packages that will be removed
            tracing::debug!(
                "*removing* following packages: {}",
                extraneous
                    .iter()
                    .map(|d| format!("{} {}", d.name(), d.version()))
                    .join(", ")
            );
        }

        if !duplicates.is_empty() {
            // List all packages that will be removed
            tracing::debug!(
                "*removing .dist-info* following duplicate packages: {}",
                duplicates
                    .iter()
                    .map(|d| format!("{} {}", d.name(), d.version()))
                    .join(", ")
            );
        }
    }

    /// This method prepares any remote distributions i.e. download and potentially build them
    async fn prepare_remote_distributions(
        &self,
        remote: &[(Dist, InstallReason)],
        setup: &UvInstallerConfig,
    ) -> miette::Result<Vec<CachedDist>> {
        let start = std::time::Instant::now();

        let options = UvReporterOptions::new()
            .with_length(remote.len() as u64)
            .with_starting_tasks(remote.iter().map(|(d, _)| format!("{}", d.name())))
            .with_top_level_message("Preparing distributions");

        let dependency_metadata = DependencyMetadata::default();
        let build_dispatch = self.create_build_dispatch(&dependency_metadata, setup);

        let distribution_database = DistributionDatabase::new(
            setup.registry_client.as_ref(),
            &build_dispatch,
            self.context_config.uv_context.concurrency.downloads,
        );

        // Before hitting the network let's make sure the credentials are available to uv
        for url in setup.index_locations.indexes().map(|index| index.url()) {
            let success = store_credentials_from_url(url.url());
            tracing::debug!("Stored credentials for {}: {}", url, success);
        }

        let preparer = Preparer::new(
            &self.context_config.uv_context.cache,
            &setup.tags,
            &uv_types::HashStrategy::None,
            &setup.build_options,
            distribution_database,
        )
        .with_reporter(UvReporter::new_arc(options));

        let resolution = Resolution::default();
        let remote_dists = preparer
            .prepare(
                remote.iter().map(|(d, _)| Arc::new(d.clone())).collect(),
                &self.context_config.uv_context.in_flight,
                &resolution,
            )
            .await
            .into_diagnostic()
            .context("Failed to prepare distributions")?;

        let s = if remote_dists.len() == 1 { "" } else { "s" };
        tracing::info!(
            "{}",
            format!(
                "Prepared {} in {}",
                format!("{} package{}", remote_dists.len(), s),
                elapsed(start.elapsed())
            )
        );

        Ok(remote_dists)
    }

    fn create_build_dispatch<'setup>(
        &'setup self,
        dependency_metadata: &'setup DependencyMetadata,
        setup: &'setup UvInstallerConfig,
    ) -> BuildDispatch<'setup>
    where
        'a: 'setup,
    {
        BuildDispatch::new(
            &setup.registry_client,
            &self.context_config.uv_context.cache,
            Constraints::default(),
            setup.venv.interpreter(),
            &setup.index_locations,
            &setup.flat_index,
            dependency_metadata,
            SharedState::default(),
            IndexStrategy::default(),
            &setup.config_settings,
            setup.build_isolation.to_uv(&setup.venv),
            LinkMode::default(),
            &setup.build_options,
            &self.context_config.uv_context.hash_strategy,
            None,
            self.context_config.uv_context.source_strategy,
            WorkspaceCache::default(),
            self.context_config.uv_context.concurrency,
            PreviewMode::Disabled,
        )
        // Important: this passes any CONDA activation to the uv build process
        .with_build_extra_env_vars(self.context_config.environment_variables.iter())
    }

    /// Remove metadata for duplicate packages
    fn remove_duplicate_metadata(&self, duplicates: &[InstalledDist]) -> std::io::Result<()> {
        for duplicate in duplicates {
            tracing::debug!(
                "Removing metadata for duplicate package: {}",
                duplicate.name()
            );
            fs_err::remove_dir_all(duplicate.install_path())?;
        }
        Ok(())
    }

    /// Remove packages that are extraneous or need reinstallation
    /// removes both packages that are unused (extraneous) and those that need reinstallation (reinstalls)
    async fn remove_packages(
        &self,
        extraneous: &[InstalledDist],
        reinstalls: &[(InstalledDist, NeedReinstall)],
    ) -> miette::Result<()> {
        if extraneous.is_empty() && reinstalls.is_empty() {
            return Ok(());
        }
        let start = std::time::Instant::now();
        for dist_info in extraneous.iter().chain(reinstalls.iter().map(|(d, _)| d)) {
            let summary = match uv_installer::uninstall(dist_info).await {
                Ok(sum) => sum,
                // Get error types from uv_installer
                Err(UninstallError::Uninstall(e))
                    if matches!(e, uv_install_wheel::Error::MissingRecord(_))
                        || matches!(e, uv_install_wheel::Error::MissingTopLevel(_)) =>
                {
                    // If the uninstallation failed, remove the directory manually and continue
                    tracing::debug!("Uninstall failed for {:?} with error: {}", dist_info, e);

                    // Sanity check to avoid calling remove all on a bad path.
                    if dist_info
                        .install_path()
                        .iter()
                        .any(|segment| Path::new(segment) == Path::new("site-packages"))
                    {
                        tokio::fs::remove_dir_all(dist_info.install_path())
                            .await
                            .into_diagnostic()?;
                    }

                    continue;
                }
                Err(err) => {
                    return Err(miette::miette!(err));
                }
            };
            tracing::debug!(
                "Uninstalled {} ({} file{}, {} director{})",
                dist_info.name(),
                summary.file_count,
                if summary.file_count == 1 { "" } else { "s" },
                summary.dir_count,
                if summary.dir_count == 1 { "y" } else { "ies" },
            );
        }

        let s = if extraneous.len() + reinstalls.len() == 1 {
            ""
        } else {
            "s"
        };
        tracing::debug!(
            "{}",
            format!(
                "Uninstalled {} in {}",
                format!("{} package{}", extraneous.len() + reinstalls.len(), s),
                elapsed(start.elapsed())
            )
        );

        Ok(())
    }

    /// Check and warn about conflicts between PyPI and Conda packages.
    /// clobbering may occur, so that a PyPI package will overwrite a conda package
    /// this method will notify the user about any potential conflicts.
    async fn check_and_warn_about_conflicts(
        &self,
        all_dists: &[CachedDist],
        reinstalls: &[(InstalledDist, NeedReinstall)],
        setup: &UvInstallerConfig,
    ) -> miette::Result<()> {
        // Determine the currently installed conda packages.
        let installed_packages =
            self.config
                .prefix
                .find_installed_packages()
                .with_context(|| {
                    format!(
                        "failed to determine the currently installed packages for {}",
                        self.config.prefix.root().display()
                    )
                })?;

        let pypi_conda_clobber = PypiCondaClobberRegistry::with_conda_packages(&installed_packages);

        // Figure what wheels needed to be re-installed because of an installer mismatch
        // we want to handle these somewhat differently and warn the user about them
        let mut installer_mismatch = reinstalls
            .iter()
            .filter_map(|(d, reason)| {
                if matches!(reason, NeedReinstall::InstallerMismatch { .. }) {
                    Some(d.name().to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        // Verify if pypi wheels will override existing conda packages and warn if they are
        if let Ok(Some(clobber_packages)) =
            pypi_conda_clobber.clobber_on_installation(all_dists.to_vec(), &setup.venv)
        {
            let packages_names = clobber_packages.iter().join(", ");

            tracing::warn!("These conda-packages will be overridden by pypi: \n\t{packages_names}");

            // because we are removing conda packages
            // we filter the ones we already warn
            if !installer_mismatch.is_empty() {
                installer_mismatch.retain(|name| !packages_names.contains(name));
            }
        }

        if !installer_mismatch.is_empty() {
            // Notify the user if there are any packages that were re-installed because they
            // were installed by a different installer.
            let packages = installer_mismatch
                .iter()
                .map(|name| name.to_string())
                .join(", ");
            // BREAK(0.20.1): change this into a warning in a future release
            tracing::info!(
                "These pypi-packages were re-installed because they were previously installed by a different installer but are currently managed by pixi: {packages}"
            )
        }

        Ok(())
    }

    /// Actually install the distributions.
    async fn install_distributions(
        &self,
        all_dists: Vec<CachedDist>,
        setup: &UvInstallerConfig,
    ) -> miette::Result<()> {
        if all_dists.is_empty() {
            return Ok(());
        }

        let options = UvReporterOptions::new()
            .with_length(all_dists.len() as u64)
            .with_starting_tasks(all_dists.iter().map(|d| format!("{}", d.name())))
            .with_top_level_message("Installing distributions");

        let start = std::time::Instant::now();

        uv_installer::Installer::new(&setup.venv)
            .with_link_mode(LinkMode::default())
            .with_installer_name(Some(consts::PIXI_UV_INSTALLER.to_string()))
            .with_reporter(UvReporter::new_arc(options))
            .install(all_dists.clone())
            .await
            .expect("should be able to install all distributions");

        let s = if all_dists.len() == 1 { "" } else { "s" };
        tracing::info!(
            "{}",
            format!(
                "Installed {} in {}",
                format!("{} package{}", all_dists.len(), s),
                elapsed(start.elapsed())
            )
        );

        Ok(())
    }
}

