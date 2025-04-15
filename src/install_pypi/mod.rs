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
use pixi_manifest::SystemRequirements;
use pixi_record::PixiRecord;
use pixi_uv_conversions::{
    isolated_names_to_packages, locked_indexes_to_index_locations, names_to_build_isolation,
    no_build_to_build_options,
};
use pypi_modifiers::{
    pypi_tags::{get_pypi_tags, is_python_record},
    Tags,
};
use rattler_conda_types::Platform;
use rattler_lock::{PypiIndexes, PypiPackageData, PypiPackageEnvironmentData};
use utils::elapsed;
use uv_auth::store_credentials_from_url;
use uv_client::{Connectivity, FlatIndexClient, RegistryClient, RegistryClientBuilder};
use uv_configuration::{BuildOptions, ConfigSettings, Constraints, IndexStrategy, PreviewMode};
use uv_dispatch::{BuildDispatch, SharedState};
use uv_distribution::{DistributionDatabase, RegistryWheelIndex};
use uv_distribution_types::{DependencyMetadata, IndexLocations, Name, Resolution};
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

use plan::{InstallPlanner, InstallReason, NeedReinstall, PyPIInstalls, PyPIRemovals};

pub(crate) mod conda_pypi_clobber;
pub(crate) mod conversions;
pub(crate) mod install_wheel;
pub(crate) mod plan;
pub(crate) mod utils;

type CombinedPypiPackageData = (PypiPackageData, PypiPackageEnvironmentData);

pub struct PyPIPrefixUpdater<'a> {
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
    non_isolated_packages: Option<Vec<uv_pep508::PackageName>>,
    environment_variables: HashMap<String, String>,
}

impl<'a> PyPIPrefixUpdater<'a> {
    /// Setup the installer, for installation later
    #[allow(clippy::too_many_arguments)]
    pub async fn setup(
        lock_file_dir: &Path,
        prefix: &Prefix,
        pixi_records: &[PixiRecord],
        python_interpreter_path: &Path,
        system_requirements: &SystemRequirements,
        uv_context: &'a UvResolutionContext,
        pypi_indexes: Option<&PypiIndexes>,
        environment_variables: &HashMap<String, String>,
        platform: Platform,
        non_isolated_packages: Option<Vec<String>>,
        no_build: &pixi_manifest::pypi::pypi_options::NoBuild,
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
        let build_options = no_build_to_build_options(no_build).into_diagnostic()?;

        let mut uv_client_builder = RegistryClientBuilder::new(uv_context.cache.clone())
            .allow_insecure_host(uv_context.allow_insecure_host.clone())
            .index_urls(index_locations.index_urls())
            .keyring(uv_context.keyring_provider)
            .connectivity(Connectivity::Online)
            .extra_middleware(uv_context.extra_middleware.clone());

        for p in &uv_context.proxies {
            uv_client_builder = uv_client_builder.proxy(p.clone())
        }

        let registry_client = Arc::new(uv_client_builder.build());

        // Resolve the flat indexes from `--find-links`.
        let flat_index = {
            let client = FlatIndexClient::new(&registry_client, &uv_context.cache);
            let indexes = index_locations.flat_indexes().map(|index| index.url());
            let entries = client.fetch(indexes).await.into_diagnostic()?;
            FlatIndex::from_entries(
                entries,
                Some(&tags),
                &uv_context.hash_strategy, // Use hash strategy from context
                &build_options,
            )
        };

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
        // Clone the input `non_isolated_packages` to potentially store or use locally.
        let owned_non_isolated_packages = non_isolated_packages.clone();
        let uv_non_isolated_packages =
            isolated_names_to_packages(owned_non_isolated_packages.as_deref()).into_diagnostic()?;

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
            non_isolated_packages: uv_non_isolated_packages, // Store the calculated build isolation
            environment_variables: environment_variables.clone(),
        })
    }

    /// Installs and/or removes python distributions.
    pub async fn update(&self, python_packages: &[CombinedPypiPackageData]) -> miette::Result<()> {
        let start = std::time::Instant::now();
        // Determine if we need to build any packages in isolation
        let build_isolation =
            names_to_build_isolation(self.non_isolated_packages.as_deref(), &self.venv);

        let dep_metadata = DependencyMetadata::default();
        let build_dispatch = BuildDispatch::new(
            &self.registry_client,
            &self.uv_context.cache,
            Constraints::default(),
            self.venv.interpreter(),
            &self.index_locations,
            &self.flat_index,
            &dep_metadata,
            SharedState::default(),
            IndexStrategy::default(),
            &self.config_settings,
            build_isolation,
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
        .with_build_extra_env_vars(self.environment_variables.iter());

        // This is used to find wheels that are available from the registry
        let registry_index = RegistryWheelIndex::new(
            &self.uv_context.cache,
            &self.tags,
            &self.index_locations,
            &HashStrategy::None,
            &self.config_settings,
        );

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

        let _lock = self
            .venv
            .lock()
            .await
            .into_diagnostic()
            .with_context(|| "error locking installation directory")?;

        // Find out what packages are already installed
        let site_packages =
            SitePackages::from_environment(&self.venv).expect("could not create site-packages");

        tracing::debug!(
            "Constructed site-packages with {} packages",
            site_packages.iter().count(),
        );

        let PyPIRemovals { extraneous } =
            InstallPlanner::new(self.uv_context.cache.clone(), &self.lock_file_dir)
                .plan_removals(&site_packages, &required_map)
                .into_diagnostic()
                .context("error while determining PyPI removal plan")?;

        // Partition into those that should be linked from the cache (`local`), those
        // that need to be downloaded (`remote`)
        let PyPIInstalls {
            local,
            remote,
            reinstalls,
        } = InstallPlanner::new(self.uv_context.cache.clone(), &self.lock_file_dir)
            .plan_install(&site_packages, registry_index, &required_map)
            .into_diagnostic()
            .context("error while determining PyPI installation plan")?;

        // Determine the currently installed conda packages.
        let installed_packages = self.prefix.find_installed_packages().with_context(|| {
            format!(
                "failed to determine the currently installed packages for {}",
                self.prefix.root().display()
            )
        })?;

        let pypi_conda_clobber = PypiCondaClobberRegistry::with_conda_packages(&installed_packages);

        // Show totals
        let total_to_install = local.len() + remote.len();
        let total_required = required_map.len();
        tracing::info!(
            "{} of {} required packages are considered are installed and up-to-date",
            total_required - total_to_install,
            total_required
        );
        // Nothing to do.
        if remote.is_empty() && local.is_empty() && reinstalls.is_empty() && extraneous.is_empty() {
            tracing::info!(
                "{}",
                format!("Nothing to do - finished in {}", elapsed(start.elapsed()))
            );
            return Ok(());
        }

        // Installation and re-installation have needed a lot of debugging in the past
        // That's why we do a bit more extensive logging here
        // This is a bit verbose but it is very helpful when debugging
        // Not enable `-vv` to get the full debug output for reinstallation
        let mut install_cached = vec![];
        let mut install_stale = vec![];
        let mut install_missing = vec![];

        // Filter out the re-installs, mostly these are less interesting than the actual re-install reasons
        // do show the installs
        for (dist, reason) in local.iter() {
            match reason {
                InstallReason::InstallStaleLocal => install_stale.push(dist.name().to_string()),
                InstallReason::InstallMissing => install_missing.push(dist.name().to_string()),
                InstallReason::InstallCached => install_cached.push(dist.name().to_string()),
                _ => {}
            }
        }
        for (dist, reason) in remote.iter() {
            match reason {
                InstallReason::InstallStaleLocal => install_stale.push(dist.name().to_string()),
                InstallReason::InstallMissing => install_missing.push(dist.name().to_string()),
                InstallReason::InstallCached => install_cached.push(dist.name().to_string()),
                _ => {}
            }
        }

        if !install_missing.is_empty() {
            tracing::info!(
                "*installing* from remote because no version is cached: {}",
                install_missing.iter().join(", ")
            );
        }
        if !install_stale.is_empty() {
            tracing::info!(
                "*installing* from remote because local version is stale: {}",
                install_stale.iter().join(", ")
            );
        }
        if !install_cached.is_empty() {
            tracing::info!(
                "*installing* cached version because cache is up-to-date: {}",
                install_cached.iter().join(", ")
            );
        }

        if !reinstalls.is_empty() {
            tracing::info!(
                "*re-installing* following packages: {}",
                reinstalls.iter().map(|(d, _)| d.name()).join(", ")
            );
            // List all re-install reasons
            for (dist, reason) in &reinstalls {
                // Only log the re-install reason if it is not an installer mismatch
                if !matches!(reason, NeedReinstall::InstallerMismatch { .. }) {
                    tracing::info!(
                        "re-installing '{}' because: '{}'",
                        console::style(dist.name()).blue(),
                        reason
                    );
                }
            }
        }
        if !extraneous.is_empty() {
            // List all packages that will be removed
            tracing::info!(
                "*removing* following packages: {}",
                extraneous.iter().map(|d| d.name()).join(", ")
            );
        }

        // Download, build, and unzip any missing distributions.
        let remote_dists = if remote.is_empty() {
            Vec::new()
        } else {
            let start = std::time::Instant::now();

            let options = UvReporterOptions::new()
                .with_length(remote.len() as u64)
                .with_starting_tasks(remote.iter().map(|(d, _)| format!("{}", d.name())))
                .with_top_level_message("Preparing distributions");

            let distribution_database = DistributionDatabase::new(
                self.registry_client.as_ref(),
                &build_dispatch,
                self.uv_context.concurrency.downloads,
            );

            // Before hitting the network let's make sure the credentials are available to
            // uv
            for url in self.index_locations.indexes().map(|index| index.url()) {
                let success = store_credentials_from_url(url);
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

            remote_dists
        };

        // Remove any unnecessary packages.
        if !extraneous.is_empty() || !reinstalls.is_empty() {
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
        }

        // Install the resolved distributions.
        // At this point we have all the wheels we need to install available to link locally
        let local_dists = local.iter().map(|(d, _)| d.clone());
        let all_dists = remote_dists
            .into_iter()
            .chain(local_dists)
            .collect::<Vec<_>>();

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

        // Verify if pypi wheels will override existing conda packages
        // and warn if they are
        if let Ok(Some(clobber_packages)) =
            pypi_conda_clobber.clobber_on_installation(all_dists.clone(), &self.venv)
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
            tracing::info!("These pypi-packages were re-installed because they were previously installed by a different installer but are currently managed by pixi: {packages}")
        }

        let options = UvReporterOptions::new()
            .with_length(all_dists.len() as u64)
            .with_starting_tasks(all_dists.iter().map(|d| format!("{}", d.name())))
            .with_top_level_message("Installing distributions");

        if !all_dists.is_empty() {
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
        }

        Ok(())
    }
}
