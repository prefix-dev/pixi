use crate::environment::PythonStatus;
use crate::prefix::Prefix;

use crate::uv_reporter::{UvReporter, UvReporterOptions};

use distribution_filename::DistFilename;

use miette::{IntoDiagnostic, WrapErr};
use uv_cache::Cache;
use uv_resolver::InMemoryIndex;

use crate::consts::PROJECT_MANIFEST;
use crate::lock_file::UvResolutionContext;
use crate::project::manifest::SystemRequirements;

use crate::pypi_tags::{get_pypi_tags, is_python_record};
use distribution_types::{CachedDist, Dist, IndexUrl, InstalledDist, Name};
use install_wheel_rs::linker::LinkMode;

use rattler_conda_types::{Platform, RepoDataRecord};
use rattler_lock::{PypiPackageData, PypiPackageEnvironmentData};

use std::time::Duration;

use uv_client::{FlatIndex, FlatIndexClient};
use uv_dispatch::BuildDispatch;
use uv_distribution::RegistryWheelIndex;
use uv_installer::{Downloader, SitePackages};
use uv_interpreter::{Interpreter, Virtualenv};
use uv_normalize::PackageName;

use uv_traits::{NoBinary, NoBuild, SetupPyStrategy};

type CombinedPypiPackageData = (PypiPackageData, PypiPackageEnvironmentData);

pub(super) fn elapsed(duration: Duration) -> String {
    let secs = duration.as_secs();

    if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else if secs > 0 {
        format!("{}.{:02}s", secs, duration.subsec_nanos() / 10_000_000)
    } else {
        format!("{}ms", duration.subsec_millis())
    }
}

/// Derived from uv [`uv_installer::Plan`]
struct PixiInstallPlan {
    /// The distributions that are not already installed in the current environment, but are
    /// available in the local cache.
    pub local: Vec<CachedDist>,

    /// The distributions that are not already installed in the current environment, and are
    /// not available in the local cache.
    /// this is where we differ from UV because we want already have the URL we want to download
    pub remote: Vec<Dist>,

    /// Any distributions that are already installed in the current environment, but will be
    /// re-installed (including upgraded) to satisfy the requirements.
    pub reinstalls: Vec<InstalledDist>,

    /// Any distributions that are already installed in the current environment, and are
    /// _not_ necessary to satisfy the requirements.
    pub extraneous: Vec<InstalledDist>,
}

/// Converts our locked data to a file
fn locked_data_to_file(pkg: &PypiPackageData, filename: &str) -> distribution_types::File {
    // Convert our url to a FileLocation
    let url = if pkg.url.scheme() == "file" {
        distribution_types::FileLocation::Path(
            pkg.url.to_file_path().expect("cannot convert to file path"),
        )
    } else {
        distribution_types::FileLocation::AbsoluteUrl(pkg.url.to_string())
    };

    // Convert PackageHashes to uv hashes
    let hashes = if let Some(ref hash) = pkg.hash {
        match hash {
            rattler_lock::PackageHashes::Md5(md5) => pypi_types::Hashes {
                md5: Some(format!("{:x}", md5)),
                sha256: None,
            },
            rattler_lock::PackageHashes::Sha256(sha256) => pypi_types::Hashes {
                md5: None,
                sha256: Some(format!("{:x}", sha256)),
            },
            rattler_lock::PackageHashes::Md5Sha256(md5, sha256) => pypi_types::Hashes {
                md5: Some(format!("{:x}", md5)),
                sha256: Some(format!("{:x}", sha256)),
            },
        }
    } else {
        pypi_types::Hashes {
            md5: None,
            sha256: None,
        }
    };

    distribution_types::File {
        filename: filename.to_string(),
        dist_info_metadata: None,
        hashes,
        requires_python: pkg.requires_python.clone(),
        upload_time_utc_ms: None,
        yanked: None,
        size: None,
        url,
    }
}

fn convert_to_dist(pkg: &PypiPackageData) -> Dist {
    // Extract last component from url
    let filename_raw = pkg.url.path_segments().unwrap().last().unwrap();
    let filename = DistFilename::try_from_normalized_filename(filename_raw).expect(&format!(
        "{} - could not convert to dist filename",
        pkg.name.as_ref()
    ));

    // Bit of a hack to create the file type
    let file = locked_data_to_file(pkg, filename_raw);

    Dist::from_registry(filename, file, IndexUrl::Pypi)
}

/// Figure out what we can link from the cache locally
/// and what we need to download from the registry.
/// Also determine what we need to remove.
/// Ignores re-installs for now.
fn whats_the_plan<'venv, 'a>(
    required: &'a [CombinedPypiPackageData],
    installed: &SitePackages<'venv>,
    registry_index: &'a mut RegistryWheelIndex<'a>,
    uv_cache: &Cache,
) -> miette::Result<PixiInstallPlan> {
    // Create a HashSet of PackageName and Version
    let mut required_map: std::collections::HashMap<&PackageName, &PypiPackageData> =
        required.iter().map(|(pkg, _)| (&pkg.name, pkg)).collect();

    // Filter out conda packages
    // Ignore packages without an installer
    let installed = installed.iter().filter(|dist| {
        dist.installer()
            .unwrap_or_default()
            .is_some_and(|installer| installer != "conda")
    });

    let mut extraneous = vec![];
    let mut local = vec![];
    let mut remote = vec![];
    let mut reinstalls = vec![];

    // TODO: Do something with editable packages
    // TODO: Check WheelTag correctness for installed packages
    // TODO: Add source dependency support

    // Walk over all installed packages and check if they are required
    for dist in installed {
        if let Some(pkg) = required_map.remove(&dist.name()) {
            // Check if the installed version is the same as the required version
            let same_version = match dist {
                InstalledDist::Registry(reg) => reg.version == pkg.version,
                InstalledDist::Url(direct_url) => direct_url.url == pkg.url,
            };

            // Don't do anything if the version is the same
            if same_version {
                continue;
            }

            // Otherwise, we need to check if we get it remote or local
            reinstalls.push(dist.clone());

            // Check if we need to revalidate
            // In that case
            if uv_cache.must_revalidate(&pkg.name) {
                remote.push(convert_to_dist(pkg));
                continue;
            }

            // Do we have in the cache?
            let wheel = registry_index
                .get(&pkg.name)
                .find(|(version, _)| **version == pkg.version);
            if let Some((_, cached)) = wheel {
                local.push(CachedDist::Registry(cached.clone()));
            } else {
                remote.push(convert_to_dist(pkg));
            }

            // TODO(tim): we need to have special handling for DirectUrl dists
        } else {
            // We can uninstall
            extraneous.push(dist.clone());
        }
    }

    // Now we need to check if we have any packages left in the required_map
    for pkg in required_map.values() {
        // Check if we need to revalidate
        // In that case
        if uv_cache.must_revalidate(&pkg.name) {
            remote.push(convert_to_dist(pkg));
            continue;
        }

        // Do we have in the cache?
        let wheel = registry_index
            .get(&pkg.name)
            .find(|(version, _)| **version == pkg.version);
        if let Some((_, cached)) = wheel {
            local.push(CachedDist::Registry(cached.clone()));
        } else {
            remote.push(convert_to_dist(pkg));
        }
    }

    Ok(PixiInstallPlan {
        local,
        remote,
        reinstalls,
        extraneous,
    })
}

/// Installs and/or remove python distributions.
// TODO: refactor arguments in struct
#[allow(clippy::too_many_arguments)]
pub async fn update_python_distributions(
    prefix: &Prefix,
    conda_package: &[RepoDataRecord],
    python_packages: &[CombinedPypiPackageData],
    status: &PythonStatus,
    system_requirements: &SystemRequirements,
    uv_context: UvResolutionContext,
) -> miette::Result<()> {
    let start = std::time::Instant::now();
    let Some(python_info) = status.current_info() else {
        // No python interpreter in the environment, so there is nothing to do here.
        return Ok(());
    };

    let python_location = prefix.root().join(&python_info.path);

    // Determine where packages would have been installed
    let _python_version = (python_info.short_version.1 as u32, 0);

    let python_record = conda_package
        .iter()
        .find(|r| is_python_record(r))
        .ok_or_else(|| miette::miette!("could not resolve pypi dependencies because no python interpreter is added to the dependencies of the project.\nMake sure to add a python interpreter to the [dependencies] section of the {PROJECT_MANIFEST}, or run:\n\n\tpixi add python"))?;

    let platform = platform_host::Platform::current().expect("unsupported platform");
    let interpreter =
        Interpreter::query(&python_location, &platform, &uv_context.cache).into_diagnostic()?;

    // Create a custom venv
    let venv = Virtualenv::from_interpreter(interpreter, prefix.root());

    // Determine the current environment markers.
    let tags = get_pypi_tags(
        Platform::current(),
        &system_requirements,
        &python_record.package_record,
    )?;

    // Resolve the flat indexes from `--find-links`.
    let flat_index = {
        let client = FlatIndexClient::new(&uv_context.registry_client, &uv_context.cache);
        let entries = client
            .fetch(uv_context.index_locations.flat_index())
            .await
            .into_diagnostic()?;
        FlatIndex::from_entries(entries, &tags)
    };

    // Track in-flight downloads, builds, etc., across resolutions.
    let no_build = NoBuild::None;
    let no_binary = NoBinary::None;

    let in_memory_index = InMemoryIndex::default();

    // Prep the build context.
    let build_dispatch = BuildDispatch::new(
        &uv_context.registry_client,
        &uv_context.cache,
        venv.interpreter(),
        &uv_context.index_locations,
        &flat_index,
        &in_memory_index,
        &uv_context.in_flight,
        venv.python_executable(),
        SetupPyStrategy::default(),
        &no_build,
        &no_binary,
    );

    let _lock = venv.lock().into_diagnostic()?;
    // TODO: need to resolve editables?

    let installed = SitePackages::from_executable(&venv).expect("could not create site-packages");
    let mut registry_index =
        RegistryWheelIndex::new(&uv_context.cache, &tags, &uv_context.index_locations);
    // Partition into those that should be linked from the cache (`local`), those that need to be
    // downloaded (`remote`), and those that should be removed (`extraneous`).
    let PixiInstallPlan {
        local,
        remote,
        reinstalls,
        extraneous,
    } = whats_the_plan(
        python_packages,
        &installed,
        &mut registry_index,
        &uv_context.cache,
    )?;
    tracing::debug!(
        "Resolved install plan: local={}, remote={}, reinstalls={}, extraneous={}",
        local.len(),
        remote.len(),
        reinstalls.len(),
        extraneous.len()
    );

    // Nothing to do.
    if remote.is_empty() && local.is_empty() && reinstalls.is_empty() && extraneous.is_empty() {
        let s = if python_packages.len() == 1 { "" } else { "s" };
        tracing::debug!(
            "{}",
            format!(
                "Audited {} in {}",
                format!(
                    "{num_requirements} package{s}",
                    num_requirements = python_packages.len()
                ),
                elapsed(start.elapsed())
            )
        );
        return Ok(());
    }

    // Download, build, and unzip any missing distributions.
    let wheels = if remote.is_empty() {
        Vec::new()
    } else {
        let start = std::time::Instant::now();

        let options = UvReporterOptions::new()
            .with_length(remote.len() as u64)
            .with_capacity(remote.len() + 30)
            .with_starting_tasks(remote.iter().map(|d| format!("{}", d.name())))
            .with_top_level_message("Downloading");

        let downloader = Downloader::new(
            &uv_context.cache,
            &tags,
            &uv_context.registry_client,
            &build_dispatch,
        )
        .with_reporter(UvReporter::new(options));

        let wheels = downloader
            .download(remote.clone(), &uv_context.in_flight)
            .await
            .into_diagnostic()
            .context("Failed to download distributions")?;

        let s = if wheels.len() == 1 { "" } else { "s" };
        tracing::info!(
            "{}",
            format!(
                "Downloaded {} in {}",
                format!("{} package{}", wheels.len(), s),
                elapsed(start.elapsed())
            )
        );

        wheels
    };

    // Remove any unnecessary packages.
    if !extraneous.is_empty() || !reinstalls.is_empty() {
        let start = std::time::Instant::now();

        for dist_info in extraneous.iter().chain(reinstalls.iter()) {
            let summary = uv_installer::uninstall(dist_info)
                .await
                .expect("uinstall did not work");
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
    let wheels = wheels.into_iter().chain(local).collect::<Vec<_>>();
    let options = UvReporterOptions::new()
        .with_length(wheels.len() as u64)
        .with_capacity(wheels.len() + 30)
        .with_starting_tasks(wheels.iter().map(|d| format!("{}", d.name())))
        .with_top_level_message("Installing distributions");
    if !wheels.is_empty() {
        let start = std::time::Instant::now();
        uv_installer::Installer::new(&venv)
            .with_link_mode(LinkMode::default())
            .with_reporter(UvReporter::new(options))
            .install(&wheels)
            .unwrap();

        let s = if wheels.len() == 1 { "" } else { "s" };
        tracing::info!(
            "{}",
            format!(
                "Installed {} in {}",
                format!("{} package{}", wheels.len(), s),
                elapsed(start.elapsed())
            )
        );
    }

    Ok(())
}
