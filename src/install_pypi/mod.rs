use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use conda_pypi_clobber::PypiCondaClobberRegistry;
use itertools::Itertools;
use miette::{IntoDiagnostic, WrapErr};
use pixi_consts::consts;
use pixi_manifest::{SystemRequirements, pypi::pypi_options::NoBuildIsolation};
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
use uv_distribution::{BuiltWheelIndex, DistributionDatabase, RegistryWheelIndex};
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
    install_pypi::plan::CachedWheelsProvider,
    lock_file::UvResolutionContext,
    prefix::Prefix,
    uv_reporter::{UvReporter, UvReporterOptions},
};

pub(crate) mod conda_pypi_clobber;
pub(crate) mod conversions;
pub(crate) mod install_wheel;
pub(crate) mod plan;
pub(crate) mod utils;

type CombinedPypiPackageData = (PypiPackageData, PypiPackageEnvironmentData);

pub struct PyPIPrefixUpdaterBuilder<'a> {
    lock_file_dir: PathBuf,
    prefix: Prefix,
    tags: Tags,
    uv_context: &'a UvResolutionContext,
    index_locations: IndexLocations,
    build_options: BuildOptions,
    registry_client: Arc<RegistryClient>,
    flat_index: FlatIndex,
    config_settings: ConfigSettings,
    venv: PythonEnvironment,
    build_isolation: BuildIsolation,
    environment_variables: HashMap<String, String>,
}

impl<'a> PyPIPrefixUpdaterBuilder<'a> {
    /// Setup the installer, for installation later
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        lock_file_dir: &Path,
        prefix: &Prefix,
        pixi_records: &[PixiRecord],
        python_interpreter_path: &Path,
        system_requirements: &SystemRequirements,
        uv_context: &'a UvResolutionContext,
        pypi_indexes: Option<&PypiIndexes>,
        environment_variables: &HashMap<String, String>,
        platform: Platform,
        non_isolated_packages: &NoBuildIsolation,
        no_build: &pixi_manifest::pypi::pypi_options::NoBuild,
        no_binary: &pixi_manifest::pypi::pypi_options::NoBinary,
    ) -> miette::Result<Self> {
        // Determine the current environment markers.
        let python_record = pixi_records
            .iter()
            .find(|r| is_python_record(r))
            .cloned() // Clone the record to own it
            .ok_or_else(|| miette::miette!("could not resolve pypi dependencies because no python interpreter is added to the dependencies of the project.\nMake sure to add a python interpreter to the [dependencies] section of the {manifest}, or run:\n\n\tpixi add python", manifest=consts::WORKSPACE_MANIFEST))?;
        let tags = get_pypi_tags(
            platform,
            system_requirements,
            python_record.package_record(),
        )?;

        let index_locations = pypi_indexes
            .map(|indexes| locked_indexes_to_index_locations(indexes, lock_file_dir))
            .unwrap_or_else(|| Ok(IndexLocations::default()))
            .into_diagnostic()?;
        let build_options = pypi_options_to_build_options(no_build, no_binary).into_diagnostic()?;

        let mut uv_client_builder = RegistryClientBuilder::new(uv_context.cache.clone())
            .allow_insecure_host(uv_context.allow_insecure_host.clone())
            .keyring(uv_context.keyring_provider)
            .connectivity(Connectivity::Online)
            .extra_middleware(uv_context.extra_middleware.clone())
            .index_locations(&index_locations);

        for p in &uv_context.proxies {
            uv_client_builder = uv_client_builder.proxy(p.clone())
        }

        let registry_client = Arc::new(uv_client_builder.build());

        // Resolve the flat indexes from `--find-links`.
        // In UV 0.7.8, we need to fetch flat index entries from the index locations
        let flat_index_client = FlatIndexClient::new(
            registry_client.cached_client(),
            Connectivity::Online,
            &uv_context.cache,
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
            &uv_context.hash_strategy,
            &build_options,
        );

        let config_settings = ConfigSettings::default();

        // Setup the interpreter from the conda prefix
        let python_location = prefix.root().join(python_interpreter_path);
        let interpreter =
            Interpreter::query(&python_location, &uv_context.cache).into_diagnostic()?;
        tracing::debug!(
            "using python interpreter: {} from {}",
            interpreter.key(),
            interpreter.sys_prefix().display()
        );

        // Create a Python environment
        let venv = PythonEnvironment::from_interpreter(interpreter.clone()); // Clone interpreter for venv

        // Determine isolated packages based on input, converting names.
        let build_isolation = non_isolated_packages.clone().try_into().into_diagnostic()?;

        Ok(Self {
            lock_file_dir: lock_file_dir.to_path_buf(),
            prefix: prefix.clone(),
            tags,
            uv_context,
            index_locations,
            build_options,
            registry_client,
            flat_index,
            config_settings,
            venv,
            build_isolation,
            environment_variables: environment_variables.clone(),
        })
    }

    /// Builds the installation plan and creates an updater
    pub fn build(
        self,
        python_packages: &[CombinedPypiPackageData],
    ) -> miette::Result<PyPIPrefixUpdater> {
        // Create a map of the required packages
        let required_map: std::collections::HashMap<uv_normalize::PackageName, &PypiPackageData> =
            python_packages
                .iter()
                .map(|(pkg, _)| {
                    let uv_name = uv_normalize::PackageName::from_str(pkg.name.as_ref())
                        .expect("should be correct");
                    (uv_name, pkg)
                })
                .collect();

        // Find out what packages are already installed
        let site_packages =
            SitePackages::from_environment(&self.venv).expect("could not create site-packages");

        tracing::debug!(
            "Constructed site-packages with {} packages",
            site_packages.iter().count(),
        );

        // This is used to find wheels that are available from the registry
        let registry_index = RegistryWheelIndex::new(
            &self.uv_context.cache,
            &self.tags,
            &self.index_locations,
            &HashStrategy::None,
            &self.config_settings,
        );
        let built_wheel_index = BuiltWheelIndex::new(
            &self.uv_context.cache,
            &self.tags,
            &HashStrategy::None,
            &self.config_settings,
        );

        // Partition into those that should be linked from the cache (`local`), those
        // that need to be downloaded (`remote`)
        let installation_plan =
            InstallPlanner::new(self.uv_context.cache.clone(), &self.lock_file_dir)
                .plan(
                    &site_packages,
                    CachedWheelsProvider::new(registry_index, built_wheel_index),
                    &required_map,
                )
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

        // Create the updater
        let updater = PyPIPrefixUpdater {
            prefix: self.prefix,
            venv: self.venv,
            uv_context: self.uv_context.clone(),
            registry_client: self.registry_client,
            environment_variables: self.environment_variables,
            tags: self.tags,
            flat_index: self.flat_index,
            config_settings: self.config_settings,
            build_options: self.build_options,
            index_locations: self.index_locations,
            build_isolation: self.build_isolation,
            installation_plan,
        };

        Ok(updater)
    }
}

/// This installs a PyPI prefix given a specific installation plan.
pub struct PyPIPrefixUpdater {
    prefix: Prefix,
    venv: PythonEnvironment,
    uv_context: UvResolutionContext,
    registry_client: Arc<RegistryClient>,
    environment_variables: HashMap<String, String>,
    tags: Tags,
    flat_index: FlatIndex,
    config_settings: ConfigSettings,
    build_options: BuildOptions,
    index_locations: IndexLocations,
    build_isolation: BuildIsolation,
    installation_plan: PyPIInstallationPlan,
}

impl PyPIPrefixUpdater {
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

    /// Perform the actual update according to the installation plan
    pub async fn update(&self) -> miette::Result<()> {
        // Lock before performing operations
        let _lock = self
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
        } = &self.installation_plan;

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
            self.prepare_remote_distributions(remote).await?
        };

        // Remove any duplicate metadata for packages that are now owned by conda
        self.remove_duplicate_metadata(duplicates)
            .into_diagnostic()
            .wrap_err("while removing duplicate metadata")?;

        // Remove any unnecessary packages.
        self.remove_packages(extraneous, reinstalls).await?;

        // Install the resolved distributions.
        // At this point we have all the wheels we need to install available to link locally
        let local_dists = local.iter().map(|(d, _)| d.clone());
        let all_dists = remote_dists
            .into_iter()
            .chain(local_dists)
            .collect::<Vec<_>>();

        self.check_and_warn_about_conflicts(&all_dists, reinstalls)
            .await?;

        self.install_distributions(all_dists).await?;
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
    ) -> miette::Result<Vec<CachedDist>> {
        let start = std::time::Instant::now();

        let options = UvReporterOptions::new()
            .with_length(remote.len() as u64)
            .with_starting_tasks(remote.iter().map(|(d, _)| format!("{}", d.name())))
            .with_top_level_message("Preparing distributions");

        let dependency_metadata = DependencyMetadata::default();
        let build_dispatch = self.create_build_dispatch(&dependency_metadata);

        let distribution_database = DistributionDatabase::new(
            self.registry_client.as_ref(),
            &build_dispatch,
            self.uv_context.concurrency.downloads,
        );

        // Before hitting the network let's make sure the credentials are available to uv
        for url in self.index_locations.indexes().map(|index| index.url()) {
            let success = store_credentials_from_url(url.url());
            tracing::debug!("Stored credentials for {}: {}", url, success);
        }

        let preparer = Preparer::new(
            &self.uv_context.cache,
            &self.tags,
            &uv_types::HashStrategy::None,
            &self.build_options,
            distribution_database,
        )
        .with_reporter(UvReporter::new_arc(options));

        let resolution = Resolution::default();
        let remote_dists = preparer
            .prepare(
                remote.iter().map(|(d, _)| Arc::new(d.clone())).collect(),
                &self.uv_context.in_flight,
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

    fn create_build_dispatch<'a>(
        &'a self,
        dependency_metadata: &'a DependencyMetadata,
    ) -> BuildDispatch<'a> {
        BuildDispatch::new(
            &self.registry_client,
            &self.uv_context.cache,
            Constraints::default(),
            self.venv.interpreter(),
            &self.index_locations,
            &self.flat_index,
            dependency_metadata,
            SharedState::default(),
            IndexStrategy::default(),
            &self.config_settings,
            self.build_isolation.to_uv(&self.venv),
            LinkMode::default(),
            &self.build_options,
            &self.uv_context.hash_strategy,
            None,
            self.uv_context.source_strategy,
            WorkspaceCache::default(),
            self.uv_context.concurrency,
            PreviewMode::Disabled,
        )
        // ! Important this passes any CONDA activation to the uv build process
        .with_build_extra_env_vars(self.environment_variables.iter())
    }

    /// Check and warn about conflicts between PyPI and Conda packages.
    /// clobbering may occur, so that a PyPI package will overwrite a conda package
    /// this method will notify the user about any potential conflicts.
    async fn check_and_warn_about_conflicts(
        &self,
        all_dists: &[CachedDist],
        reinstalls: &[(InstalledDist, NeedReinstall)],
    ) -> miette::Result<()> {
        // Determine the currently installed conda packages.
        let installed_packages = self.prefix.find_installed_packages().with_context(|| {
            format!(
                "failed to determine the currently installed packages for {}",
                self.prefix.root().display()
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
            pypi_conda_clobber.clobber_on_installation(all_dists.to_vec(), &self.venv)
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
    async fn install_distributions(&self, all_dists: Vec<CachedDist>) -> miette::Result<()> {
        if all_dists.is_empty() {
            return Ok(());
        }

        let options = UvReporterOptions::new()
            .with_length(all_dists.len() as u64)
            .with_starting_tasks(all_dists.iter().map(|d| format!("{}", d.name())))
            .with_top_level_message("Installing distributions");

        let start = std::time::Instant::now();

        uv_installer::Installer::new(&self.venv)
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
