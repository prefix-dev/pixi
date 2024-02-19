//! This module contains code to resolve python package from PyPi or Conda packages.
//!
//! See [`resolve_pypi`] and [`resolve_conda`] for more information.

use crate::project::manifest::LibCSystemRequirement;
use crate::project::virtual_packages::{default_glibc_version, default_mac_os_version};
use crate::pypi_marker_env::determine_marker_environment;
use crate::pypi_tags::is_python_record;
use crate::{
    lock_file::{pypi, LockedCondaPackages, LockedPypiPackages, PypiRecord},
    project::manifest::{PyPiRequirement, SystemRequirements},
};
use distribution_types::{BuiltDist, Dist, IndexLocations, IndexUrls, Resolution};
use indexmap::IndexMap;
use indicatif::ProgressBar;
use miette::IntoDiagnostic;
use platform_host::{Os, Platform};
use platform_tags::Tags;
use rattler::install::PythonInfo;
use rattler_conda_types::{Arch, GenericVirtualPackage, MatchSpec, RepoDataRecord, Version};
use rattler_lock::{PackageHashes, PypiPackageData, PypiPackageEnvironmentData};
use rattler_solve::{resolvo, SolverImpl};
use std::cmp::min;
use std::path::PathBuf;
use std::{path::Path, sync::Arc};
use uv_cache::Cache;
use uv_client::{Connectivity, FlatIndex, FlatIndexClient, RegistryClient, RegistryClientBuilder};
use uv_dispatch::BuildDispatch;
use uv_interpreter::Interpreter;
use uv_normalize::PackageName;
use uv_resolver::{InMemoryIndex, Manifest, Options, Resolver};
use uv_traits::{InFlight, NoBinary, NoBuild, SetupPyStrategy};

fn convert_to_uv_platform(
    platform: rattler_conda_types::Platform,
    system_requirements: SystemRequirements,
) -> miette::Result<Platform> {
    let platform = if platform.is_linux() {
        let arch = match platform.arch() {
            None => unreachable!("every platform we support has an arch"),
            Some(Arch::X86) => platform_host::Arch::X86,
            Some(Arch::X86_64) => platform_host::Arch::X86_64,
            Some(Arch::Aarch64) => platform_host::Arch::Aarch64,
            Some(Arch::ArmV7l) => platform_host::Arch::Armv7L,
            Some(Arch::Ppc64le) => platform_host::Arch::Powerpc64Le,
            Some(Arch::Ppc64) => platform_host::Arch::Powerpc64,
            Some(Arch::S390X) => platform_host::Arch::S390X,
            Some(unsupported_arch) => {
                miette::miette!("unsupported arch for pypi packages '{unsupported_arch}'")
            }
        };

        // Find the glibc version
        match system_requirements
            .libc
            .as_ref()
            .map(LibCSystemRequirement::family_and_version)
        {
            None => {
                let (major, minor) = default_glibc_version()
                    .as_major_minor()
                    .expect("expected default glibc version to be a major.minor version");
                Platform::new(
                    Os::Manylinux {
                        major: major as _,
                        minor: minor as _,
                    },
                    arch,
                )
            }
            Some(("glibc", version)) => {
                let Some((major, minor)) = version.as_major_minor() else {
                    miette::miette!(
                        "expected glibc version to be a major.minor version, but got '{version}'"
                    )
                };
                Platform::new(
                    Os::Manylinux {
                        major: major as _,
                        minor: minor as _,
                    },
                    arch,
                )
            }
            Some((family, _)) => {
                return Err(miette::miette!(
                    "unsupported libc family for pypi packages '{family}'"
                ));
            }
        }
    } else if platform.is_windows() {
        let arch = match platform.arch() {
            None => unreachable!("every platform we support has an arch"),
            Some(Arch::X86) => platform_host::Arch::X86,
            Some(Arch::X86_64) => platform_host::Arch::X86_64,
            Some(Arch::Aarch64) => platform_host::Arch::Aarch64,
            Some(unsupported_arch) => {
                miette::miette!("unsupported arch for pypi packages '{unsupported_arch}'")
            }
        };

        Platform::new(Os::Windows, arch)
    } else if platform.is_osx() {
        let Some((major, minor)) = system_requirements
            .macos
            .unwrap_or_else(default_mac_os_version(platform))
            .as_major_minor()
        else {
            miette::miette!(
                "expected macos version to be a major.minor version, but got '{version}'"
            )
        };

        let arch = match platform.arch() {
            None => unreachable!("every platform we support has an arch"),
            Some(Arch::X86) => platform_host::Arch::X86,
            Some(Arch::X86_64) => platform_host::Arch::X86_64,
            Some(Arch::Aarch64) => platform_host::Arch::Aarch64,
            Some(unsupported_arch) => {
                miette::miette!("unsupported arch for pypi packages '{unsupported_arch}'")
            }
        };

        Platform::new(
            Os::Macos {
                major: major as _,
                minor: minor as _,
            },
            arch,
        )
    } else {
        return Err(miette::miette!(
            "unsupported platform for pypi packages {platform}"
        ));
    };

    Ok(platform)
}

/// This function takes as input a set of dependencies and system requirements and returns a set of
/// locked packages.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_pypi(
    package_db: Arc<PackageDb>,
    dependencies: IndexMap<PackageName, Vec<PyPiRequirement>>,
    system_requirements: SystemRequirements,
    locked_conda_records: &[RepoDataRecord],
    _locked_pypi_records: &[PypiRecord],
    platform: rattler_conda_types::Platform,
    pb: &ProgressBar,
    python_info: &PythonInfo,
    venv_root: &Path,
) -> miette::Result<LockedPypiPackages> {
    // Solve python packages
    pb.set_message("resolving pypi dependencies");

    let requirements = dependencies
        .iter()
        .flat_map(|(name, req)| req.iter().map(move |req| (name, req)))
        .map(|(name, req)| req.as_pep508(name))
        .collect::<Vec<pep508_rs::Requirement>>();

    // Determine the python interpreter that is installed as part of the conda packages.
    let python_record = locked_conda_records
        .iter()
        .find(|r| is_python_record(r))
        .ok_or_else(|| miette::miette!("could not resolve pypi dependencies because no python interpreter is added to the dependencies of the project.\nMake sure to add a python interpreter to the [dependencies] section of the {PROJECT_MANIFEST}, or run:\n\n\tpixi add python"))?;

    // Construct the marker environment for the target platform
    let marker_environment = determine_marker_environment(platform, python_record.as_ref())?;

    // Construct a fake interpreter from the conda environment.
    // TODO: Should we look into using the actual interpreter here?
    let interpreter = Interpreter::artificial(
        Platform::current().expect("unsupported platform"),
        marker_environment.clone(),
        venv_root.to_path_buf(),
        venv_root.to_path_buf(),
        venv_root.join(python_info.path()),
        Path::new("invalid").to_path_buf(),
    );

    // Build the wheel tags based on the interpreter, the target platform, and the python version.
    let Some(python_version) = python_record.package_record.version.as_major_minor() else {
        return Err(miette::miette!(
            "expected python version to be a major.minor version, but got '{version}'"
        ));
    };
    let implementation_name = match python_record.package_record.name.as_normalized() {
        "python" => "cpython",
        "pypy" => "pypy",
        _ => {
            return Err(miette::miette!(
                "unsupported python implementation '{}'",
                python_record.package_record.name.as_source()
            ));
        }
    };
    let target_platform = convert_to_uv_platform(platform, system_requirements)?;
    let tags = Tags::from_env(
        &target_platform,
        (
            python_info.short_version.0 as u8,
            python_info.short_version.1 as u8,
        ),
        implementation_name,
        (python_version.0 as u8, python_version.1 as u8),
    )
    .context("failed to determine the python wheel tags for the target platform")?;

    // Construct a cache
    // TODO: Figure out the right location
    let cache = Cache::temp().with_context("failed to create cache")?;

    // Define where to get packages from
    let index_locations = IndexUrls::default();

    // Construct a registry client
    let registry_client = RegistryClientBuilder::new(cache.clone())
        .index_urls(index_locations.clone())
        .connectivity(Connectivity::Online)
        .build();

    // Resolve the flat indexes from `--find-links`.
    let flat_index = {
        let client = FlatIndexClient::new(&registry_client, &cache);
        let entries = client.fetch(index_locations.flat_index()).await?;
        FlatIndex::from_entries(entries, tags)
    };

    // Create a shared in-memory index.
    let index = InMemoryIndex::default();

    // Track in-flight downloads, builds, etc., across resolutions.
    let in_flight = InFlight::default();

    let options = Options::default();

    let build_dispatch = BuildDispatch::new(
        &registry_client,
        &cache,
        &interpreter,
        &index_locations,
        &flat_index,
        &index,
        &in_flight,
        interpreter.sys_executable().to_path_buf(),
        &SetupPyStrategy::Pep517,
        &NoBuild::None,
        &NoBinary::None,
    )
    .with_options(options.clone());

    let resolution = Resolver::new(
        Manifest::simple(requirements),
        Options::default(),
        &marker_environment,
        &interpreter,
        &tags,
        &registry_client,
        &flat_index,
        index,
        &build_dispatch,
    )
    .resolve()
    .await
    .context("failed to resolve pypi dependencies")?;
    let resolution = Resolution::from(resolution);

    // Clear message
    pb.set_message("");

    // let mut locked_packages = LockedPypiPackages::with_capacity(resolution.len());
    // for dist in resolution.into_distributions() {
    //     let file_location = match dist {
    //         Dist::Built(BuiltDist::Registry(dist)) => dist,
    //         Dist::Built(BuiltDist::DirectUrl(dist)) => dist.url.to_url(),
    //         Dist::Built(BuiltDist::Path(dist)) => dist.url.to_url(),
    //         // Dist::Source(_) => {}
    //     }
    // }

    // // Add pip packages
    // let mut locked_packages = LockedPypiPackages::with_capacity(python_artifacts.len());
    // for python_artifact in python_artifacts {
    //     let (artifact, metadata) = package_db
    //         // No need for a WheelBuilder here since any builds should have been done during the
    //         // [`python::resolve_dependencies`] call.
    //         .get_metadata(&python_artifact.artifacts, None)
    //         .await
    //         .expect("failed to get metadata for a package for which we have already fetched metadata during solving.")
    //         .expect("no metadata for a package for which we have already fetched metadata during solving.");
    //
    //     let pkg_data = PypiPackageData {
    //         name: python_artifact.name.to_string(),
    //         version: python_artifact.version,
    //         requires_dist: metadata.requires_dist,
    //         requires_python: metadata.requires_python,
    //         url: artifact.url.clone(),
    //         hash: artifact
    //             .hashes
    //             .as_ref()
    //             .and_then(|hash| PackageHashes::from_hashes(None, hash.sha256)),
    //     };
    //
    //     let pkg_env = PypiPackageEnvironmentData {
    //         extras: python_artifact
    //             .extras
    //             .into_iter()
    //             .map(|e| e.as_str().to_string())
    //             .collect(),
    //     };
    //
    //     locked_packages.push((pkg_data, pkg_env));
    // }

    let locked_packages = vec![];

    Ok(locked_packages)
}

/// Solves the conda package environment for the given input. This function is async because it
/// spawns a background task for the solver. Since solving is a CPU intensive task we do not want to
/// block the main task.
pub async fn resolve_conda(
    specs: Vec<MatchSpec>,
    virtual_packages: Vec<GenericVirtualPackage>,
    locked_packages: Vec<RepoDataRecord>,
    available_packages: Vec<Vec<RepoDataRecord>>,
) -> miette::Result<LockedCondaPackages> {
    tokio::task::spawn_blocking(move || {
        // Construct a solver task that we can start solving.
        let task = rattler_solve::SolverTask {
            specs,
            available_packages: &available_packages,
            locked_packages,
            pinned_packages: vec![],
            virtual_packages,
            timeout: None,
        };

        // Solve the task
        resolvo::Solver.solve(task).into_diagnostic()
    })
    .await
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(e) => std::panic::resume_unwind(e),
        Err(_err) => Err(miette::miette!("cancelled")),
    })
}
