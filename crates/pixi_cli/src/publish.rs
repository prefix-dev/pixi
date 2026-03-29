use std::path::PathBuf;

use clap::Parser;
use indicatif::ProgressBar;
use miette::{Context, IntoDiagnostic};
use pixi_auth::get_auth_store;
use pixi_build_frontend::BackendOverride;
use pixi_command_dispatcher::{
    BuildBackendMetadataSpec, BuildEnvironment, BuildProfile, CacheDirs, SourceBuildSpec,
    build::PinnedSourceCodeLocation,
};
use pixi_config::{Config, ConfigCli};
use pixi_core::{WorkspaceLocator, environment::sanity_check_workspace};
use pixi_manifest::FeaturesExt;
use pixi_path::AbsPathBuf;
use pixi_progress::global_multi_progress;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec};
use pixi_reporters::TopLevelProgress;
use pixi_utils::variants::VariantConfig;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_networking::AuthenticationStorage;
use rattler_package_streaming::seek::read_package_file;

use crate::build::{determine_discovery_start, validate_package_manifest};
use crate::cli_config::LockAndInstallConfig;

/// Build a conda package and publish it to a channel.
///
/// This is a convenience command that combines `pixi build` and `pixi upload`.
///
/// Supported target channel URLs:
///   - prefix.dev: `https://prefix.dev/<channel-name>`
///   - anaconda.org: `https://anaconda.org/<owner>/<label>`
///   - S3: `s3://bucket-name`
///   - Filesystem: `file:///path/to/channel`
///   - Quetz: `quetz://server/<channel>`
///   - Artifactory: `artifactory://server/<channel>`
#[derive(Parser, Debug)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub config_cli: ConfigCli,

    /// Backend override for testing purposes.
    #[clap(skip)]
    pub backend_override: Option<BackendOverride>,

    #[clap(flatten)]
    pub lock_and_install_config: LockAndInstallConfig,

    /// The target platform to build for (defaults to the current platform)
    #[clap(long, short, default_value_t = Platform::current())]
    pub target_platform: Platform,

    /// The build platform to use for building (defaults to the current platform)
    #[clap(long, default_value_t = Platform::current())]
    pub build_platform: Platform,

    /// The directory to use for incremental builds artifacts.
    #[clap(long, short)]
    pub build_dir: Option<PathBuf>,

    /// Whether to clean the build directory before building.
    #[clap(long, short)]
    pub clean: bool,

    /// The path to a directory containing a package manifest, or to a specific manifest file.
    ///
    /// Supported manifest files: `package.xml`, `recipe.yaml`, `pixi.toml`, `pyproject.toml`, or `mojoproject.toml`.
    #[arg(long)]
    pub path: Option<PathBuf>,

    /// The target channel URL to publish packages to.
    ///
    /// Examples:
    ///   --to <https://prefix.dev/my-channel>
    ///   --to <https://anaconda.org/my-user>
    ///   --to s3://my-bucket/my-channel
    ///   --to file:///path/to/local/channel
    #[arg(long)]
    pub to: String,

    /// Force overwrite existing packages
    #[arg(long)]
    pub force: bool,

    /// Skip uploading packages that already exist on the target channel.
    /// This is enabled by default. Use `--no-skip-existing` to disable.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub skip_existing: bool,

    /// Generate sigstore attestation (prefix.dev only)
    #[arg(long)]
    pub generate_attestation: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // === Phase 1: Build the packages (same logic as `pixi build`) ===

    let workspace_locator = determine_discovery_start(&args.path).await?;

    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(workspace_locator.clone())
        .with_closest_package(false)
        .locate()?
        .with_cli_config(args.config_cli);

    sanity_check_workspace(&workspace).await?;

    let multi_progress = global_multi_progress();
    let anchor_pb = multi_progress.add(ProgressBar::hidden());
    let cache_dir = AbsPathBuf::new(pixi_config::get_cache_dir()?)
        .expect("cache dir is not absolute")
        .into_assume_dir();
    let workspace_dir = AbsPathBuf::new(workspace.pixi_dir())
        .expect("pixi dir is not absolute")
        .into_assume_dir();
    let mut cache_dirs = CacheDirs::new(cache_dir).with_workspace(workspace_dir);
    if let Some(build_dir) = args.build_dir {
        let build_dir = AbsPathBuf::new(build_dir)
            .expect("build dir is not absolute")
            .into_assume_dir();
        cache_dirs.set_working_dirs(build_dir);
    }
    let command_dispatcher = workspace
        .command_dispatcher_builder()?
        .with_cache_dirs(cache_dirs)
        .with_reporter(TopLevelProgress::new(multi_progress, anchor_pb))
        .finish();

    let VariantConfig {
        variant_configuration,
        variant_files,
    } = workspace.variants(args.target_platform)?;

    let build_virtual_packages: Vec<GenericVirtualPackage> = workspace
        .default_environment()
        .virtual_packages(args.build_platform)
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();

    let host_virtual_packages: Vec<GenericVirtualPackage> = workspace
        .default_environment()
        .virtual_packages(args.target_platform)
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();

    let build_environment = BuildEnvironment {
        host_platform: args.target_platform,
        build_platform: args.build_platform,
        build_virtual_packages,
        host_virtual_packages,
    };

    let Ok(manifest_path) = workspace_locator.path() else {
        miette::bail!("could not determine the current working directory to locate the workspace");
    };

    let package_manifest_path = match args.path {
        Some(path) => {
            validate_package_manifest(&path).await?;
            path
        }
        None => manifest_path.clone(),
    };

    let package_manifest_path_canonical = dunce::canonicalize(&package_manifest_path)
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to canonicalize manifest path '{}'",
                package_manifest_path.display()
            )
        })?;

    let manifest_path_spec =
        pathdiff::diff_paths(&package_manifest_path_canonical, workspace.root())
            .unwrap_or_else(|| package_manifest_path_canonical.to_path_buf());

    let channel_config = workspace.channel_config();
    let channels = workspace
        .default_environment()
        .channel_urls(&channel_config)
        .into_diagnostic()?;
    let exclude_newer = workspace
        .default_environment()
        .exclude_newer_config_resolved(&channel_config, Some(build_environment.host_platform))
        .into_diagnostic()?;

    let manifest_source: PinnedSourceSpec = PinnedPathSpec {
        path: manifest_path_spec.to_string_lossy().into_owned().into(),
    }
    .into();

    let backend_metadata_spec = BuildBackendMetadataSpec {
        manifest_source: manifest_source.clone(),
        preferred_build_source: None,
        channels: channels.clone(),
        channel_config: channel_config.clone(),
        exclude_newer: exclude_newer.clone(),
        build_environment: build_environment.clone(),
        variant_configuration: Some(variant_configuration.clone()),
        variant_files: Some(variant_files.clone()),
        enabled_protocols: Default::default(),
    };
    let backend_metadata = command_dispatcher
        .build_backend_metadata(backend_metadata_spec.clone())
        .await?;

    let packages = backend_metadata.metadata.outputs();

    // Print initial build summary
    pixi_progress::println!(
        "\n{}Building {} package(s):",
        console::style(console::Emoji("📋 ", "")).cyan(),
        packages.len()
    );
    for pkg in &packages {
        pixi_progress::println!(
            "  - {} v{} [{}] ({})",
            pkg.name.as_normalized(),
            pkg.version,
            pkg.build,
            pkg.subdir
        );
    }
    pixi_progress::println!("");

    // Build and collect all package paths
    let mut built_package_paths: Vec<PathBuf> = Vec::new();

    for package in packages {
        let built_package = command_dispatcher
            .source_build(SourceBuildSpec {
                package,
                output_directory: None,
                source: PinnedSourceCodeLocation::new(manifest_source.clone(), None),
                channels: channels.clone(),
                exclude_newer: exclude_newer.clone(),
                channel_config: channel_config.clone(),
                build_environment: build_environment.clone(),
                variant_configuration: Some(variant_configuration.clone()),
                variant_files: Some(variant_files.clone()),
                variants: None,
                enabled_protocols: Default::default(),
                work_directory: None,
                clean: args.clean,
                force: false,
                build_profile: BuildProfile::Release,
            })
            .await?;

        command_dispatcher.clear_reporter().await;

        let package_path = dunce::canonicalize(&built_package.output_file)
            .expect("failed to canonicalize output file which must now exist");

        built_package_paths.push(package_path);
    }

    if built_package_paths.is_empty() {
        miette::bail!("No packages were built. Nothing to publish.");
    }

    // === Phase 2: Upload the built packages ===

    pixi_progress::println!(
        "\n{}Publishing {} package(s) to {}",
        console::style(console::Emoji("📦 ", "")).cyan(),
        built_package_paths.len(),
        args.to
    );

    let config = Config::load_global();
    let auth_storage = get_auth_store(&config).into_diagnostic()?;

    let target_url = parse_target_url(&args.to)?;

    pixi_progress::await_in_progress("uploading packages", |_| {
        upload_packages(
            &target_url,
            &built_package_paths,
            &auth_storage,
            args.force,
            args.skip_existing,
            args.generate_attestation,
        )
    })
    .await?;

    pixi_progress::println!(
        "{}Successfully published {} package(s) to {}",
        console::style(console::Emoji("✔ ", "")).green(),
        built_package_paths.len(),
        args.to
    );
    for path in &built_package_paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        pixi_progress::println!("  - {}", name);
    }

    Ok(())
}

/// Parse a target URL string into a URL, handling various schemes.
fn parse_target_url(to: &str) -> miette::Result<url::Url> {
    url::Url::parse(to).map_err(|e| miette::miette!("Invalid target URL '{}': {}", to, e))
}

/// Determine the subdirectory (platform) of a conda package.
fn determine_package_subdir(package_path: &std::path::Path) -> miette::Result<String> {
    let index_json: rattler_conda_types::package::IndexJson = read_package_file(package_path)
        .map_err(|e| miette::miette!("Failed to read package file: {}", e))?;

    Ok(index_json.subdir.unwrap_or_else(|| "noarch".to_string()))
}

/// Upload packages to the target channel based on the URL scheme/host.
///
/// This logic is adapted from `rattler_build_core::publish::upload_and_index_channel`.
async fn upload_packages(
    url: &url::Url,
    package_paths: &[PathBuf],
    auth_storage: &AuthenticationStorage,
    force: bool,
    skip_existing: bool,
    generate_attestation: bool,
) -> miette::Result<()> {
    let scheme = url.scheme();

    match scheme {
        "s3" => upload_to_s3(url, package_paths, auth_storage, force).await,
        "quetz" => upload_to_quetz(url, package_paths, auth_storage).await,
        "artifactory" => upload_to_artifactory(url, package_paths, auth_storage).await,
        "prefix" => {
            upload_to_prefix(
                url,
                package_paths,
                auth_storage,
                force,
                skip_existing,
                generate_attestation,
            )
            .await
        }
        "file" => {
            let path = url
                .to_file_path()
                .map_err(|()| miette::miette!("Invalid file URL: {}", url))?;
            upload_to_local_filesystem(&path, package_paths, force, skip_existing).await
        }
        "http" | "https" => {
            let host = url.host_str().unwrap_or("");

            if host.contains("prefix.dev") {
                upload_to_prefix(
                    url,
                    package_paths,
                    auth_storage,
                    force,
                    skip_existing,
                    generate_attestation,
                )
                .await
            } else if host.contains("anaconda.org") {
                upload_to_anaconda(url, package_paths, auth_storage, force).await
            } else if host.contains("quetz") {
                upload_to_quetz(url, package_paths, auth_storage).await
            } else {
                Err(miette::miette!(
                    "Cannot determine upload backend from URL '{}'. \n\
                    Supported hosts: prefix.dev, anaconda.org, or use explicit schemes: s3://, quetz://, artifactory://, prefix://",
                    url
                ))
            }
        }
        _ => Err(miette::miette!(
            "Unsupported URL scheme '{}'. Supported schemes: file://, s3://, quetz://, artifactory://, prefix://, http://, https://",
            scheme
        )),
    }
}

/// Upload packages to a Prefix.dev server.
async fn upload_to_prefix(
    url: &url::Url,
    package_paths: &[PathBuf],
    auth_storage: &AuthenticationStorage,
    force: bool,
    skip_existing: bool,
    generate_attestation: bool,
) -> miette::Result<()> {
    use rattler_upload::upload::opt::{
        AttestationSource, ForceOverwrite, PrefixData, SkipExisting,
    };
    use rattler_upload::upload::upload_package_to_prefix;

    tracing::info!("Uploading packages to Prefix.dev: {}", url);

    let channel = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .ok_or_else(|| miette::miette!("Invalid Prefix URL: missing channel name"))?
        .to_string();

    let mut server_url = url.clone();
    if server_url.scheme() == "prefix" {
        server_url
            .set_scheme("https")
            .map_err(|_| miette::miette!("Failed to convert prefix:// URL to https://"))?;
    }
    server_url.set_path("");

    let attestation = if generate_attestation {
        AttestationSource::GenerateAttestation
    } else {
        AttestationSource::NoAttestation
    };

    let prefix_data = PrefixData::new(
        server_url,
        channel,
        None,
        attestation,
        SkipExisting(skip_existing),
        ForceOverwrite(force),
        false,
    );

    upload_package_to_prefix(auth_storage, &package_paths.to_vec(), prefix_data)
        .await
        .into_diagnostic()
}

/// Upload packages to Anaconda.org.
async fn upload_to_anaconda(
    url: &url::Url,
    package_paths: &[PathBuf],
    auth_storage: &AuthenticationStorage,
    force: bool,
) -> miette::Result<()> {
    use rattler_upload::upload::opt::{AnacondaData, ForceOverwrite};
    use rattler_upload::upload::upload_package_to_anaconda;

    tracing::info!("Uploading packages to Anaconda.org: {}", url);

    let path_segments: Vec<&str> = url
        .path_segments()
        .ok_or_else(|| miette::miette!("Invalid Anaconda.org URL: missing path"))?
        .collect();

    let (owner, channel) = match path_segments.len() {
        1 => (path_segments[0].to_string(), "main".to_string()),
        2 => (path_segments[0].to_string(), path_segments[1].to_string()),
        _ => {
            return Err(miette::miette!(
                "Invalid Anaconda.org URL format. Expected: https://anaconda.org/owner or https://anaconda.org/owner/label"
            ));
        }
    };

    let anaconda_data = AnacondaData::new(
        owner,
        Some(vec![channel]),
        None,
        None,
        ForceOverwrite(force),
    );

    upload_package_to_anaconda(auth_storage, &package_paths.to_vec(), anaconda_data)
        .await
        .into_diagnostic()
}

/// Upload packages to a Quetz server.
async fn upload_to_quetz(
    url: &url::Url,
    package_paths: &[PathBuf],
    auth_storage: &AuthenticationStorage,
) -> miette::Result<()> {
    use rattler_upload::upload::opt::QuetzData;
    use rattler_upload::upload::upload_package_to_quetz;

    tracing::info!("Uploading packages to Quetz: {}", url);

    let channel = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .ok_or_else(|| miette::miette!("Invalid Quetz URL: missing channel name"))?
        .to_string();

    let mut server_url = url.clone();
    if server_url.scheme() == "quetz" {
        server_url
            .set_scheme("https")
            .map_err(|_| miette::miette!("Failed to convert quetz:// URL to https://"))?;
    }
    server_url.set_path("");

    let quetz_data = QuetzData::new(server_url, channel, None);

    upload_package_to_quetz(auth_storage, &package_paths.to_vec(), quetz_data).await
}

/// Upload packages to an Artifactory server.
async fn upload_to_artifactory(
    url: &url::Url,
    package_paths: &[PathBuf],
    auth_storage: &AuthenticationStorage,
) -> miette::Result<()> {
    use rattler_upload::upload::opt::ArtifactoryData;
    use rattler_upload::upload::upload_package_to_artifactory;

    tracing::info!("Uploading packages to Artifactory: {}", url);

    let channel = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .ok_or_else(|| miette::miette!("Invalid Artifactory URL: missing repository name"))?
        .to_string();

    let mut server_url = url.clone();
    if server_url.scheme() == "artifactory" {
        server_url
            .set_scheme("https")
            .map_err(|_| miette::miette!("Failed to convert artifactory:// URL to https://"))?;
    }
    server_url.set_path("");

    let artifactory_data = ArtifactoryData::new(server_url, channel, None);

    upload_package_to_artifactory(auth_storage, &package_paths.to_vec(), artifactory_data).await
}

/// Upload packages to S3 and run indexing.
async fn upload_to_s3(
    url: &url::Url,
    package_paths: &[PathBuf],
    auth_storage: &AuthenticationStorage,
    force: bool,
) -> miette::Result<()> {
    use rattler_index::{IndexS3Config, ensure_channel_initialized_s3, index_s3};
    use rattler_upload::upload::upload_package_to_s3;
    use std::collections::HashSet;

    tracing::info!("Uploading packages to S3: {}", url);

    // Resolve S3 credentials using AWS SDK default credential chain
    let resolved_credentials = rattler_s3::ResolvedS3Credentials::from_sdk()
        .await
        .map_err(|e| miette::miette!("Failed to resolve S3 credentials: {}", e))?;

    ensure_channel_initialized_s3(url, &resolved_credentials)
        .await
        .map_err(|e| miette::miette!("Failed to initialize S3 channel: {}", e))?;

    let mut subdirs = HashSet::new();
    for package_path in package_paths {
        let subdir = determine_package_subdir(package_path)?;
        subdirs.insert(subdir);
    }

    upload_package_to_s3(
        auth_storage,
        url.clone(),
        None,
        &package_paths.to_vec(),
        force,
    )
    .await?;

    tracing::info!("Successfully uploaded packages to S3, running indexing...");

    for subdir in subdirs {
        let target_platform = subdir
            .parse::<Platform>()
            .map_err(|e| miette::miette!("Invalid platform subdir '{}': {}", subdir, e))?;

        let index_config = IndexS3Config {
            channel: url.clone(),
            credentials: resolved_credentials.clone(),
            target_platform: Some(target_platform),
            repodata_patch: None,
            write_zst: true,
            write_shards: true,
            force: false,
            max_parallel: std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(1),
            multi_progress: None,
            precondition_checks: rattler_index::PreconditionChecks::Enabled,
        };

        index_s3(index_config)
            .await
            .map_err(|e| miette::miette!("Failed to index S3 channel: {}", e))?;
    }

    Ok(())
}

/// Upload packages to local filesystem and run indexing.
async fn upload_to_local_filesystem(
    target_dir: &std::path::Path,
    package_paths: &[PathBuf],
    force: bool,
    skip_existing: bool,
) -> miette::Result<()> {
    use rattler_index::{IndexFsConfig, ensure_channel_initialized_fs, index_fs};
    use std::collections::HashSet;

    tracing::info!(
        "Publishing packages to local channel: {}",
        target_dir.display()
    );

    fs_err::create_dir_all(target_dir).into_diagnostic()?;

    ensure_channel_initialized_fs(target_dir)
        .await
        .map_err(|e| miette::miette!("Failed to initialize local channel: {}", e))?;

    let mut subdirs = HashSet::new();

    for package_path in package_paths {
        let package_name = package_path
            .file_name()
            .ok_or_else(|| miette::miette!("Invalid package path"))?;

        let subdir = determine_package_subdir(package_path)?;
        subdirs.insert(subdir.clone());
        let target_subdir = target_dir.join(&subdir);

        fs_err::create_dir_all(&target_subdir).into_diagnostic()?;
        let target_path = target_subdir.join(package_name);

        if target_path.exists() {
            if skip_existing {
                pixi_progress::println!(
                    "{}Skipping '{}' (already exists)",
                    console::style(console::Emoji("⏭ ", "")).yellow(),
                    package_name.to_string_lossy()
                );
                continue;
            }
            if !force {
                return Err(miette::miette!(
                    "Package already exists at {}. Use --force to overwrite.",
                    target_path.display()
                ));
            }
        }

        tracing::info!(
            "Copying {} to {}",
            package_path.display(),
            target_path.display()
        );
        fs_err::copy(package_path, &target_path).into_diagnostic()?;
    }

    tracing::info!("Indexing local channel at {}", target_dir.display());

    for subdir in subdirs {
        let target_platform = subdir
            .parse::<Platform>()
            .map_err(|e| miette::miette!("Invalid platform subdir '{}': {}", subdir, e))?;

        let index_config = IndexFsConfig {
            channel: target_dir.to_path_buf(),
            target_platform: Some(target_platform),
            repodata_patch: None,
            write_zst: true,
            write_shards: true,
            force: false,
            max_parallel: std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(1),
            multi_progress: None,
        };

        index_fs(index_config)
            .await
            .map_err(|e| miette::miette!("Failed to index channel: {}", e))?;
    }

    Ok(())
}
