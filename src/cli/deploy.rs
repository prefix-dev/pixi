use crate::build::BuildContext;
use crate::environment::{
    EnvironmentFile, LockedEnvironmentHash, read_environment_file, update_prefix_conda,
    update_prefix_pypi, verify_prefix_location_unchanged, write_environment_file,
};
use crate::lock_file::virtual_packages::validate_system_meets_environment_requirements;
use crate::lock_file::{IoConcurrencyLimit, UvResolutionContext};
use crate::prefix::Prefix;
use crate::workspace::best_platform;
use clap::Parser;
use fancy_display::FancyDisplay;
use futures::TryFutureExt;
use indicatif::ProgressBar;
use itertools::Itertools;
use miette::{IntoDiagnostic, WrapErr};
use pixi_command_dispatcher::{CacheDirs, CommandDispatcher};
use pixi_config::{Config, ConfigCli};
use pixi_consts::consts;
use pixi_manifest::EnvironmentName;
use pixi_manifest::pypi::pypi_options::{NoBuild, NoBuildIsolation};
use pixi_progress::global_multi_progress;
use pixi_record::PixiRecord;
use pixi_utils::reqwest::build_reqwest_clients;
use rattler_conda_types::{Arch, ChannelConfig, ChannelUrl, Platform};
use rattler_lock::LockFile;
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use std::path::PathBuf;
use url::Url;

/// Install an environment in a lockfile to a target directory.
#[derive(Parser, Debug)]
pub struct Args {
    /// The environment to install
    #[arg(long, short)]
    pub environment: Option<String>,

    /// The .lock file to install
    #[arg(long, short, default_value = "pixi.lock")]
    pub lockfile: PathBuf,

    /// The target directory to install to
    #[arg(required = true)]
    pub target: PathBuf,

    #[clap(flatten)]
    pub config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let lockfile = LockFile::from_path(args.lockfile.as_path()).into_diagnostic()?;
    let lockfile_abs_path = args.lockfile.canonicalize().into_diagnostic()?; // lockfile exists
    let lockfile_dir = lockfile_abs_path
        .parent()
        .expect("parent of lockfile path must exist");

    let name = args
        .environment
        .map(EnvironmentName::Named)
        .unwrap_or_default();

    let env = lockfile.environment(name.as_str());
    let mut plat = Platform::current();

    if let Some(env) = env {
        plat = best_platform(
            env.platforms().collect(),
            args.target.join(consts::CONDA_META_DIR),
        );

        if !env.platforms().contains(&plat) {
            return Err(miette::miette!(format!(
                "No platform {} defined for the environment {} in {}",
                plat,
                name,
                args.lockfile.display()
            )));
        }

        // Validate the virtual packages for the environment match the system
        validate_system_meets_environment_requirements(&lockfile, plat, &name, None)?;
    } else {
        return Err(miette::miette!(format!(
            "No environment {} on platform {} in {}",
            name,
            plat,
            args.lockfile.display()
        )));
    }

    let env = env.expect("env must exist");
    let plat = plat;
    let hash = LockedEnvironmentHash::from_environment(env, plat);

    if args.target.exists() {
        let target_dir = args.target.canonicalize().into_diagnostic()?;
        verify_prefix_location_unchanged(target_dir.as_path()).await?;
    }

    if let Ok(Some(environment_file)) = read_environment_file(args.target.as_path()) {
        if environment_file.environment_lock_file_hash == hash {
            // If we contain source packages from conda or PyPI we update the prefix by default
            let contains_conda_source_pkgs = env
                .conda_packages(plat)
                .is_some_and(|mut packages| packages.any(|package| package.as_source().is_some()));

            // Check if we have source packages from PyPI that is local path
            let contains_pypi_source_pkgs = env.pypi_packages(plat).is_some_and(|mut packages| {
                packages.any(|(package, _)| package.location.as_path().is_some())
            });

            if contains_conda_source_pkgs || contains_pypi_source_pkgs {
                tracing::debug!(
                    "Lock file contains source packages: ignore lock file hash and update the prefix"
                );
            } else {
                tracing::info!(
                    "Environment '{}' is up-to-date with lock file hash",
                    name.fancy_display()
                );
                return Ok(());
            }
        }
    } else {
        tracing::debug!(
            "Environment file not found or parsable for '{}'",
            name.fancy_display()
        );
    }

    // git credentials are stripped in the lockfile

    let config = Config::with_cli_config(&args.config);
    let client = build_reqwest_clients(None, None)?.1;

    // Install or update
    fs_err::create_dir_all(&args.target).ok();
    let target_dir = args.target.canonicalize().into_diagnostic()?;
    let prefix = Prefix::new(target_dir.clone()); // must use absolute path
    tracing::info!("Updating prefix: '{}'", target_dir.display());

    let pixi_records = env
        .conda_packages(plat)
        .map(|iter| {
            iter.cloned()
                .map(PixiRecord::try_from)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
        .into_diagnostic()?
        .unwrap_or_default();

    // Spawn a task to determine the currently installed packages.
    let prefix_cloned = prefix.clone();
    let installed_packages_future = tokio::task::spawn_blocking(move || {
        prefix_cloned.find_installed_packages().into_diagnostic()
    })
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(_err) => Err(miette::miette!("the operation was cancelled")),
    });

    // Wait until the conda records are available and until the installed packages
    // for this prefix are available.
    let installed_packages = installed_packages_future.await?;
    let has_existing_packages = !installed_packages.is_empty();

    // Construct a command dispatcher that will be used to run the tasks.
    let command_dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(CacheDirs::new(pixi_config::get_cache_dir()?))
        .with_gateway(config.gateway().with_client(client.clone()).finish())
        .with_download_client(client.clone())
        .with_max_download_concurrency(config.max_concurrent_downloads())
        .with_reporter(crate::reporters::TopLevelProgress::new(
            global_multi_progress(),
            global_multi_progress().add(ProgressBar::hidden()),
        ))
        .finish();

    // Update the prefix with conda packages
    let python_status = update_prefix_conda(
        &prefix,
        command_dispatcher.package_cache().clone(),
        client.clone(),
        installed_packages,
        pixi_records.clone(),
        VirtualPackages::detect(&VirtualPackageOverrides::default())
            .into_diagnostic()?
            .into_generic_virtual_packages()
            .collect(),
        env.channels()
            .iter()
            .filter_map(|c| Url::parse(c.url.as_str()).ok())
            .map(ChannelUrl::from)
            .collect(),
        plat,
        &format!(
            "{} conda prefix '{}'",
            if has_existing_packages {
                "updating"
            } else {
                "creating"
            },
            name.fancy_display()
        ),
        "  ",
        IoConcurrencyLimit::default().into(), // default io limit
        BuildContext::new(
            ChannelConfig::default_with_root_dir(lockfile_dir.into()),
            Default::default(), // default variant
            command_dispatcher,
        )
        .into_diagnostic()?,
        None,
        Default::default(), /* disable post link scripts */
    )
    .await?;

    // No `uv` support for WASM right now
    if plat.arch() == Some(Arch::Wasm32) {
        return Ok(());
    }

    let pypi_records = env
        .pypi_packages(plat)
        .map(|iter| {
            iter.map(|(data, env_data)| (data.clone(), env_data.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let uv_context = UvResolutionContext::new(&config)?.set_cache_refresh(Some(false), None);

    // Update the prefix with Pypi records
    update_prefix_pypi(
        &name,
        &prefix,
        plat,
        &pixi_records,
        &pypi_records,
        &python_status,
        &Default::default(), // use default system requirements
        &uv_context,
        env.pypi_indexes(),
        &Default::default(), // skip get_activated_environment_variables
        lockfile_dir,
        plat,
        &NoBuildIsolation::All, // enable no-build-isolation for all packages
        &NoBuild::All,          // enable no-build for all packages
    )
    .await
    .with_context(|| {
        format!(
            "Failed to update PyPI packages for environment '{}'",
            name.fancy_display()
        )
    })?;

    // Save an environment file to the environment directory after the update.
    // Avoiding writing the cache away before the update is done.
    write_environment_file(
        &args.target,
        EnvironmentFile {
            manifest_path: PathBuf::new(), // no manifest
            environment_name: name.to_string(),
            pixi_version: consts::PIXI_VERSION.to_string(),
            environment_lock_file_hash: hash,
        },
    )?;

    Ok(())
}
