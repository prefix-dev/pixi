use std::collections::HashMap;
use std::io::{Cursor, Read as _};
use std::path::{Path, PathBuf};

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
use pixi_utils::reqwest::build_reqwest_clients;
use pixi_utils::variants::VariantConfig;
use rattler_conda_types::{
    Channel, GenericVirtualPackage, MatchSpec, PackageName, PackageNameMatcher, Platform,
    package::IndexJson,
};
use rattler_networking::AuthenticationStorage;
use rattler_package_streaming::seek::read_package_file;
use rattler_conda_types::compression_level::CompressionLevel;

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

    /// Override the build number for all outputs.
    ///
    /// Use an absolute value (e.g., `--build-number=12`) to set the build
    /// number directly, or a relative bump (e.g., `--build-number=+1`) to
    /// increment from the highest build number currently on the target channel.
    #[arg(long)]
    pub build_number: Option<String>,
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

    let manifest_source: PinnedSourceSpec = PinnedPathSpec {
        path: manifest_path_spec.to_string_lossy().into_owned().into(),
    }
    .into();

    let backend_metadata_spec = BuildBackendMetadataSpec {
        manifest_source: manifest_source.clone(),
        preferred_build_source: None,
        channels: channels.clone(),
        channel_config: channel_config.clone(),
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

    // === Phase 1.5: Apply build number override if requested ===

    let config = Config::load_global();
    let target_url = parse_target_url(&args.to)?;

    if let Some(ref build_number_arg) = args.build_number {
        let build_number_override = BuildNumberOverride::parse(build_number_arg)?;

        let highest_build_numbers = match &build_number_override {
            BuildNumberOverride::Relative(_) => {
                pixi_progress::await_in_progress(
                    "fetching channel repodata for build number bump",
                    |_| {
                        fetch_highest_build_numbers(
                            &config,
                            &target_url,
                            &built_package_paths,
                        )
                    },
                )
                .await?
            }
            BuildNumberOverride::Absolute(num) => {
                tracing::info!("Setting build number to {} for all outputs", num);
                HashMap::new()
            }
        };

        built_package_paths = apply_build_number_override(
            &built_package_paths,
            &build_number_override,
            &highest_build_numbers,
        )?;
    }

    // === Phase 2: Upload the built packages ===

    pixi_progress::println!(
        "\n{}Publishing {} package(s) to {}",
        console::style(console::Emoji("📦 ", "")).cyan(),
        built_package_paths.len(),
        args.to
    );

    let auth_storage = get_auth_store(&config).into_diagnostic()?;

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

// === Build Number Override ===

/// Specifies how to override the build number.
#[derive(Debug, Clone)]
enum BuildNumberOverride {
    /// Set an absolute build number (e.g., `12`).
    Absolute(u64),
    /// Apply a relative bump from the highest existing build number (e.g., `+1`).
    Relative(i64),
}

impl BuildNumberOverride {
    fn parse(s: &str) -> miette::Result<Self> {
        let s = s.trim();
        if let Some(stripped) = s.strip_prefix('+') {
            let bump: i64 = stripped
                .parse()
                .map_err(|e| miette::miette!("Invalid relative build number '{}': {}", s, e))?;
            Ok(BuildNumberOverride::Relative(bump))
        } else if s.starts_with('-') {
            let bump: i64 = s
                .parse()
                .map_err(|e| miette::miette!("Invalid relative build number '{}': {}", s, e))?;
            Ok(BuildNumberOverride::Relative(bump))
        } else {
            let num: u64 = s
                .parse()
                .map_err(|e| miette::miette!("Invalid absolute build number '{}': {}", s, e))?;
            Ok(BuildNumberOverride::Absolute(num))
        }
    }
}

/// Update a build string's trailing build number.
///
/// The build string format is typically `{hash}_{build_number}`, e.g.
/// `h1234abc_0`. This replaces the number after the last `_` with the new
/// build number.
fn update_build_string(build_string: &str, new_build_number: u64) -> String {
    if let Some(pos) = build_string.rfind('_') {
        format!("{}_{}", &build_string[..pos], new_build_number)
    } else {
        format!("{}_{}", build_string, new_build_number)
    }
}

/// Fetch the highest build numbers for packages from the target channel's
/// repodata.
async fn fetch_highest_build_numbers(
    config: &Config,
    target_url: &url::Url,
    package_paths: &[PathBuf],
) -> miette::Result<HashMap<(PackageName, String), u64>> {
    // Read metadata from each built package
    let mut package_infos = Vec::new();
    let mut platforms = std::collections::HashSet::new();
    let mut match_specs = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    for package_path in package_paths {
        let index_json: IndexJson = read_package_file(package_path)
            .map_err(|e| miette::miette!("Failed to read package '{}': {}", package_path.display(), e))?;
        let platform_str = index_json.subdir.as_deref().unwrap_or("noarch");
        let platform: Platform = platform_str
            .parse()
            .map_err(|e| miette::miette!("Invalid platform '{}': {}", platform_str, e))?;
        platforms.insert(platform);
        platforms.insert(Platform::NoArch);

        if seen_names.insert(index_json.name.clone()) {
            match_specs.push(MatchSpec {
                name: PackageNameMatcher::Exact(index_json.name.clone()),
                ..Default::default()
            });
        }
        package_infos.push((index_json.name.clone(), index_json.version.to_string()));
    }

    let platforms_vec: Vec<Platform> = platforms.into_iter().collect();

    // Convert the target URL to a channel URL suitable for repodata queries
    let channel_url = target_channel_to_repodata_url(target_url)?;
    let channel = Channel::from_url(channel_url);

    let (_, client) = build_reqwest_clients(Some(config), None)?;
    let gateway = config.gateway().with_client(client).finish();

    let repo_data = match gateway
        .query([channel], platforms_vec, match_specs)
        .recursive(false)
        .execute()
        .await
    {
        Ok(data) => data,
        Err(e) => {
            tracing::debug!("Failed to fetch repodata from target channel: {e}. Using 0 as base build number.");
            return Ok(HashMap::new());
        }
    };

    let mut highest: HashMap<(PackageName, String), u64> = HashMap::new();
    for repo in &repo_data {
        for record in repo.iter() {
            let key = (
                record.package_record.name.clone(),
                record.package_record.version.version().to_string(),
            );
            let build_number = record.package_record.build_number;
            highest
                .entry(key)
                .and_modify(|current| *current = (*current).max(build_number))
                .or_insert(build_number);
        }
    }

    Ok(highest)
}

/// Convert a target publish URL to a channel URL suitable for repodata queries.
///
/// For prefix.dev and anaconda.org URLs, the publish URL is already the channel
/// URL. For `prefix://` scheme, it's converted to `https://`.
fn target_channel_to_repodata_url(target_url: &url::Url) -> miette::Result<url::Url> {
    let mut url = target_url.clone();
    match url.scheme() {
        "prefix" => {
            url.set_scheme("https")
                .map_err(|_| miette::miette!("Failed to convert prefix:// URL to https://"))?;
            Ok(url)
        }
        "http" | "https" => Ok(url),
        "file" => Ok(url),
        scheme => Err(miette::miette!(
            "Relative build number override (e.g., +1) is not supported with '{}://' channels. \
             Use an absolute build number instead (e.g., --build-number=0).",
            scheme
        )),
    }
}

/// Apply a build number override to all built packages, repacking them with the
/// new build number and build string.
fn apply_build_number_override(
    package_paths: &[PathBuf],
    build_number_override: &BuildNumberOverride,
    highest_build_numbers: &HashMap<(PackageName, String), u64>,
) -> miette::Result<Vec<PathBuf>> {
    let mut new_paths = Vec::with_capacity(package_paths.len());

    for package_path in package_paths {
        let index_json: IndexJson = read_package_file(package_path)
            .map_err(|e| miette::miette!("Failed to read package '{}': {}", package_path.display(), e))?;

        let new_build_number = match build_number_override {
            BuildNumberOverride::Absolute(num) => *num,
            BuildNumberOverride::Relative(bump) => {
                let key = (
                    index_json.name.clone(),
                    index_json.version.to_string(),
                );
                let current_highest = highest_build_numbers.get(&key).copied().unwrap_or(0);
                let result = current_highest as i64 + bump;
                let clamped = result.max(0) as u64;
                tracing::info!(
                    "Bumping build number for {} v{}: {} + ({}) = {}",
                    index_json.name.as_normalized(),
                    index_json.version,
                    current_highest,
                    bump,
                    clamped
                );
                clamped
            }
        };

        let new_build_string = update_build_string(&index_json.build, new_build_number);

        pixi_progress::println!(
            "  {} v{}: build {} -> {} ({})",
            index_json.name.as_normalized(),
            index_json.version,
            index_json.build,
            new_build_string,
            new_build_number
        );

        let new_path = repack_with_build_number(
            package_path,
            &index_json,
            new_build_number,
            &new_build_string,
        )?;
        new_paths.push(new_path);
    }

    Ok(new_paths)
}

/// Repack a `.conda` package with a new build number and build string.
///
/// This extracts the package contents, modifies `info/index.json`, and repacks
/// everything into a new `.conda` file. The original file is removed if the
/// output path differs.
fn repack_with_build_number(
    package_path: &Path,
    index_json: &IndexJson,
    new_build_number: u64,
    new_build_string: &str,
) -> miette::Result<PathBuf> {
    if !package_path
        .extension()
        .is_some_and(|ext| ext == "conda")
    {
        miette::bail!(
            "Build number override is only supported for .conda packages, got: {}",
            package_path.display()
        );
    }

    let temp_dir = tempfile::tempdir().into_diagnostic()?;
    let extract_dir = temp_dir.path();

    // Extract the .conda file (a ZIP containing .tar.zst members)
    extract_conda_package(package_path, extract_dir)?;

    // Modify info/index.json
    let index_json_path = extract_dir.join("info").join("index.json");
    let index_content = fs_err::read_to_string(&index_json_path).into_diagnostic()?;
    let mut index: serde_json::Value =
        serde_json::from_str(&index_content).into_diagnostic()?;
    index["build_number"] = serde_json::Value::from(new_build_number);
    index["build"] = serde_json::Value::from(new_build_string);
    let new_index_content = serde_json::to_string_pretty(&index).into_diagnostic()?;
    fs_err::write(&index_json_path, new_index_content).into_diagnostic()?;

    // Collect all file paths for repacking
    let mut all_paths = Vec::new();
    collect_files_recursive(extract_dir, &mut all_paths)?;

    // Create the new package file
    let out_name = format!(
        "{}-{}-{}",
        index_json.name.as_normalized(),
        index_json.version,
        new_build_string
    );
    let output_filename = format!("{}.conda", out_name);
    let output_dir = package_path
        .parent()
        .ok_or_else(|| miette::miette!("Package path has no parent directory"))?;
    let output_path = output_dir.join(&output_filename);

    let output_file = fs_err::File::create(&output_path).into_diagnostic()?;

    rattler_package_streaming::write::write_conda_package(
        output_file,
        extract_dir,
        &all_paths,
        CompressionLevel::Default,
        None,
        &out_name,
        None,
        None,
    )
    .into_diagnostic()
    .with_context(|| format!("Failed to repack package as '{}'", output_path.display()))?;

    // Remove the original package if the path changed
    if output_path != package_path {
        fs_err::remove_file(package_path).into_diagnostic()?;
    }

    Ok(output_path)
}

/// Extract a `.conda` package (ZIP with `.tar.zst` members) to a directory.
fn extract_conda_package(package_path: &Path, dest: &Path) -> miette::Result<()> {
    let file = fs_err::File::open(package_path).into_diagnostic()?;
    let mut zip = zip::ZipArchive::new(file).into_diagnostic()?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).into_diagnostic()?;
        let name = entry.name().to_string();

        if name.ends_with(".tar.zst") {
            // Read the compressed entry into memory
            let mut compressed_data = Vec::new();
            entry
                .read_to_end(&mut compressed_data)
                .into_diagnostic()?;

            // Decompress zstd
            let decompressed = zstd::decode_all(Cursor::new(&compressed_data))
                .into_diagnostic()
                .with_context(|| format!("Failed to decompress '{}'", name))?;

            // Extract tar
            let mut tar = tar::Archive::new(Cursor::new(decompressed));
            tar.unpack(dest)
                .into_diagnostic()
                .with_context(|| format!("Failed to untar '{}'", name))?;
        }
    }

    Ok(())
}

/// Recursively collect all file paths in a directory.
fn collect_files_recursive(dir: &Path, paths: &mut Vec<PathBuf>) -> miette::Result<()> {
    for entry in fs_err::read_dir(dir).into_diagnostic()? {
        let entry = entry.into_diagnostic()?;
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, paths)?;
        } else {
            paths.push(path);
        }
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

