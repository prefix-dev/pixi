use fs_err::tokio as tokio_fs;
use pixi_consts::consts::{
    MOJOPROJECT_MANIFEST, PYPROJECT_MANIFEST, RATTLER_BUILD_FILE_NAMES, ROS_BACKEND_FILE_NAMES,
    WORKSPACE_MANIFEST,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::OsStr,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};
use url::Url;

use clap::Parser;
use indicatif::ProgressBar;
use miette::{Context, IntoDiagnostic};
use pixi_auth::get_auth_store;
use pixi_build_frontend::BackendOverride;
use pixi_command_dispatcher::{
    BackendMetadataDir, BuildBackendMetadataSpec, BuildEnvironment, BuildProfile, CacheDirs,
    ComputeResultExt, EnvironmentRef, EnvironmentSpec, EphemeralEnv,
    keys::{ResolveSourcePackageKey, ResolveSourcePackageSpec, SourceBuildKey, SourceBuildSpec},
};
use pixi_config::ConfigCli;
use pixi_core::{
    Workspace, WorkspaceLocator, environment::sanity_check_workspace, workspace::DiscoveryStart,
};
use pixi_manifest::{FeaturesExt, S3Options};
use pixi_path::AbsPathBuf;
use pixi_progress::global_multi_progress;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec};
use pixi_reporters::TopLevelProgress;
use pixi_spec::SourceLocationSpec;
use pixi_utils::variants::VariantConfig;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_networking::{AuthenticationStorage, s3_middleware};
use rattler_package_streaming::seek::read_package_file;

/// Build a conda package and publish it to a channel.
///
/// This is a convenience command that combines `pixi build` and `pixi upload`.
///
/// Supported target URLs (--target-channel / --to):
///   - prefix.dev: `https://prefix.dev/<channel-name>`
///   - anaconda.org: `https://anaconda.org/<owner>/<label>`
///   - S3: `s3://bucket-name`
///   - Local channel (with indexing): `channel:///path/to/channel`
///   - Local path (copy only): `file:///path/to/output`
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

    /// The target platform to build for (defaults to the current platform)
    #[clap(long, short, default_value_t = Platform::current())]
    pub target_platform: Platform,

    /// The build platform to use for building (defaults to the current platform)
    #[clap(long, default_value_t = Platform::current())]
    pub build_platform: Platform,

    /// An optional prefix prepended to the auto-generated build string.
    #[clap(long)]
    pub build_string_prefix: Option<String>,

    /// An optional override for the package's build number.
    #[clap(long)]
    pub build_number: Option<u64>,

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
    ///   <https://prefix.dev/my-channel>
    ///   <https://anaconda.org/my-user>
    ///   s3://my-bucket/my-channel
    ///   channel:///path/to/local/channel
    ///   file:///path/to/local/channel
    #[arg(long, visible_alias = "to", conflicts_with = "target_dir")]
    pub target_channel: Option<String>,

    /// The target local directory to copy packages into (no channel indexing).
    ///
    /// Accepts a local filesystem path.  Mutually exclusive with `--target-channel`.
    #[arg(long, conflicts_with = "target_channel")]
    pub target_dir: Option<PathBuf>,

    /// Force overwrite existing packages
    #[arg(long)]
    pub force: bool,

    /// Skip uploading packages that already exist at the target.
    /// This is enabled by default. Use `--no-skip-existing` to disable.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub skip_existing: bool,

    /// Generate sigstore attestation (prefix.dev only)
    #[arg(long)]
    pub generate_attestation: bool,
}

/// Resolved inputs that the upload dispatch needs to pick a backend and talk
/// to it.
///
/// Built once from the workspace + CLI args, then passed by reference to
/// `upload_packages_to_channel` and the per-backend helpers so each one can
/// pull only what it needs (S3 bucket options, credentials, behavior flags).
pub struct PublishContext {
    /// S3 bucket options merged from the global config, any workspace-local
    /// config files, and the manifest's `[workspace.s3-options]` table.
    /// Manifest entries override config-file entries for the same bucket.
    pub s3_options: HashMap<String, s3_middleware::S3Config>,

    /// Credential lookup used by every non-S3 backend (prefix.dev, anaconda,
    /// quetz, artifactory) and as the access-key source when S3 credentials
    /// are not provided directly.
    pub auth_storage: AuthenticationStorage,

    /// Overwrite packages that already exist on the target channel.
    pub force: bool,

    /// Skip packages that already exist on the target channel.
    pub skip_existing: bool,

    /// Request a sigstore attestation for the upload (prefix.dev only).
    pub generate_attestation: bool,
}

impl PublishContext {
    pub fn new(
        workspace: &Workspace,
        force: bool,
        skip_existing: bool,
        generate_attestation: bool,
    ) -> miette::Result<Self> {
        let config = workspace.config();

        let s3_options = merge_s3_options(
            config.compute_s3_config(),
            workspace.workspace.value.workspace.s3_options.as_ref(),
        );

        Ok(Self {
            s3_options,
            auth_storage: get_auth_store(config).into_diagnostic()?,
            force,
            skip_existing,
            generate_attestation,
        })
    }
}

/// Merge config-file `s3_options` with manifest `[workspace.s3-options]`.
///
/// Manifest entries override config entries for the same bucket name.
fn merge_s3_options(
    mut base: HashMap<String, s3_middleware::S3Config>,
    manifest: Option<&HashMap<String, S3Options>>,
) -> HashMap<String, s3_middleware::S3Config> {
    let Some(manifest) = manifest else {
        return base;
    };
    for (bucket, opts) in manifest {
        base.insert(
            bucket.clone(),
            s3_middleware::S3Config::Custom {
                endpoint_url: opts.endpoint_url.clone(),
                region: opts.region.clone(),
                force_path_style: opts.force_path_style,
            },
        );
    }
    base
}

/// Validate that the full path of package manifest exists and is a supported
/// format. Directories are allowed (for discovery), and specific manifest files
/// must be supported formats.
async fn validate_package_manifest(path: &PathBuf) -> miette::Result<()> {
    let supported_file_names: Vec<&str> = [
        // backend-specific build files
        // that will be autodiscovered
        &ROS_BACKEND_FILE_NAMES[..],
        &RATTLER_BUILD_FILE_NAMES[..],
        // manifests that can contain a package section in it
        &[WORKSPACE_MANIFEST],
        &[PYPROJECT_MANIFEST],
        &[MOJOPROJECT_MANIFEST],
    ]
    .concat();

    // we dont allow for now passing directories without a manifest file
    // from the list below
    let unsupported_implicit_file_names: Vec<&str> = [&ROS_BACKEND_FILE_NAMES[..]].concat();

    // Iterate over the files in the directory to provide a more helpful error
    // of what manifests were found.
    if path.is_dir() {
        let mut entries = tokio_fs::read_dir(&path).await.into_diagnostic()?;

        while let Some(entry) = entries.next_entry().await.into_diagnostic()? {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                if unsupported_implicit_file_names.contains(&filename) {
                    return Err(miette::diagnostic!(
                        help = format!("did you mean {filename}?"),
                        "the build manifest path '{}' is a directory, please provide the path to the manifest file",
                        path.display(),
                    ).into());
                }

                // we found a supported manifest file
                // which means that we will let our backend discovery handle it
                if supported_file_names.contains(&filename) {
                    return Ok(());
                }
            }
        }

        let supported_names = supported_file_names.join(", ");
        return Err(miette::diagnostic!(
            help = format!(
                "Ensure that the source directory contains a valid manifest file: {supported_names}"
            ),
            "'{}' does not contain a supported build manifest",
            path.display(),
        )
        .into());
    } else {
        let filename = path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| miette::miette!("Failed to extract file name from {:?}", path))?;

        if !supported_file_names
            .iter()
            .any(|names| names.contains(filename))
        {
            let supported_names = supported_file_names
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
                .join(", ");

            return Err(miette::diagnostic!(
                help = format!("Supported formats are: {supported_names}"),
                "the build manifest file '{}' is not a supported format.",
                path.display(),
            )
            .into());
        }
    }

    Ok(())
}

async fn determine_discovery_start(path: &Option<PathBuf>) -> miette::Result<DiscoveryStart> {
    match path {
        Some(path) => {
            // We need to solve the path to an absolute path
            // because we can point to specific package manifest file
            // but still want to discover the workspace from the package location.
            // For this, we need to take the parent directory of the package manifest file
            // which `WorkspaceLocator` will use to discover the workspace.
            let resolved_path = if path.is_relative() {
                std::env::current_dir().into_diagnostic()?.join(path)
            } else {
                path.to_path_buf()
            };

            // If it's a directory, use it as the search root
            if resolved_path.is_dir() {
                Ok(DiscoveryStart::SearchRoot(resolved_path))
            } else {
                // If it's a file, use its parent directory as the search root
                let package_dir = resolved_path.parent().ok_or_else(|| {
                    miette::miette!("Failed to get parent directory of package manifest")
                })?;
                Ok(DiscoveryStart::SearchRoot(package_dir.to_path_buf()))
            }
        }
        // If no path is provided, use the current directory
        None => Ok(DiscoveryStart::CurrentDir),
    }
}

enum UrlOrPath {
    Url(Url),
    Path(PathBuf),
}

impl fmt::Display for UrlOrPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UrlOrPath::Url(url) => write!(f, "{url}"),
            UrlOrPath::Path(path) => write!(f, "{}", path.display()),
        }
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // === Phase 1: Build the packages (same logic as `pixi build`) ===

    let workspace_locator = determine_discovery_start(&args.path).await?;

    let mut workspace = WorkspaceLocator::for_cli()
        .with_search_start(workspace_locator.clone())
        .with_closest_package(false)
        .locate()?
        .with_cli_config(args.config_cli);
    if let Some(backend_override) = args.backend_override.clone() {
        workspace = workspace.with_backend_override(backend_override);
    }

    sanity_check_workspace(&workspace).await?;

    let ctx = PublishContext::new(
        &workspace,
        args.force,
        args.skip_existing,
        args.generate_attestation,
    )?;

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
        cache_dirs.set_override::<BackendMetadataDir>(build_dir);
    }
    let progress = std::sync::Arc::new(TopLevelProgress::new(
        pixi_compute_reporters::OperationRegistry::new(),
        multi_progress,
        anchor_pb,
    ));
    let command_dispatcher = progress
        .clone()
        .register_with(
            workspace
                .command_dispatcher_builder()?
                .with_cache_dirs(cache_dirs),
        )
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

    // When running `pixi publish`, the exclude_newer config is ignored;
    // it only matters when using the package as a source dependency.
    let env_ref = EnvironmentRef::Ephemeral(EphemeralEnv::new(
        manifest_source.to_string(),
        EnvironmentSpec {
            channels: channels.clone(),
            build_environment: build_environment.clone(),
            variants: pixi_utils::variants::VariantConfig {
                variant_configuration: variant_configuration.clone(),
                variant_files: variant_files.clone(),
            },
            exclude_newer: None,
            channel_priority: Default::default(),
        },
    ));
    let backend_metadata_spec = BuildBackendMetadataSpec {
        manifest_source: manifest_source.clone(),
        preferred_build_source: None,
        env_ref: env_ref.clone(),
        build_string_prefix: args.build_string_prefix.clone(),
        build_number: args.build_number,
    };
    let backend_metadata = command_dispatcher
        .build_backend_metadata(backend_metadata_spec.clone())
        .await?;

    let packages = &backend_metadata.metadata.outputs;

    // Print initial build summary
    pixi_progress::println!(
        "\n{}Building {} package(s):",
        console::style(console::Emoji("📋 ", "")).cyan(),
        packages.len()
    );
    for pkg in packages {
        pixi_progress::println!(
            "  - {} v{} [{}] ({})",
            pkg.metadata.name.as_normalized(),
            pkg.metadata.version,
            pkg.metadata.build,
            pkg.metadata.subdir
        );
    }
    pixi_progress::println!("");

    // Pre-resolve a SourceRecord per unique package name via RSP; each
    // returned variant becomes a separate SourceBuildKey invocation.
    let unique_names: BTreeSet<_> = packages.iter().map(|p| p.metadata.name.clone()).collect();
    let source_location: SourceLocationSpec = manifest_source.clone().into();
    let mut resolved_records = Vec::new();
    for name in unique_names {
        let rsp = ResolveSourcePackageSpec {
            package: name,
            source_location: source_location.clone(),
            preferred_build_source: Arc::new(BTreeMap::new()),
            env_ref: env_ref.clone(),
            installed_source_hints: Default::default(),
        };
        let records = command_dispatcher
            .engine()
            .compute(&ResolveSourcePackageKey::new(rsp))
            .await
            .map_err_into_dispatcher(std::convert::identity)
            .into_diagnostic()?;
        resolved_records.extend(records.iter().cloned());
    }

    // `--clean` nukes the per-package artifact + workspace caches so
    // the upcoming SourceBuildKey calls rebuild from scratch.
    if args.clean {
        for record in &resolved_records {
            command_dispatcher
                .clear_source_build_cache(&record.data.package_record.name)
                .into_diagnostic()?;
        }
    }

    // Build and collect all package paths
    let mut built_package_paths: Vec<PathBuf> = Vec::new();

    for record in resolved_records {
        let record = Arc::unwrap_or_clone(record);
        let build_spec = SourceBuildSpec {
            record: Arc::new(record.into()),
            channels: channels.clone(),
            exclude_newer: None,
            build_environment: build_environment.clone(),
            build_profile: BuildProfile::Release,
            variant_configuration: Some(variant_configuration.clone()),
            variant_files: Some(variant_files.clone()),
            build_string_prefix: args.build_string_prefix.clone(),
            build_number: args.build_number,
        };
        let built = command_dispatcher
            .engine()
            .compute(&SourceBuildKey::new(build_spec))
            .await
            .map_err_into_dispatcher(std::convert::identity)
            .into_diagnostic()?;

        progress.on_clear();

        let package_path = dunce::canonicalize(&built.artifact)
            .expect("failed to canonicalize output file which must now exist");

        built_package_paths.push(package_path);
    }

    if built_package_paths.is_empty() {
        miette::bail!("No packages were built. Nothing to publish.");
    }

    let base = std::env::current_dir()
        .into_diagnostic()
        .context("Could not get current work directory.")?;

    let target = match (args.target_channel, args.target_dir) {
        (Some(channel), None) => {
            Ok::<UrlOrPath, miette::Error>(UrlOrPath::Url(parse_target(&channel, base.as_path())?))
        }
        (None, Some(dir)) => Ok(UrlOrPath::Path(dir)),
        (None, None) => Ok(UrlOrPath::Path(base)),
        (Some(_), Some(_)) => unreachable!("clap enforces mutual exclusion"),
    }?;

    // === Phase 2: Upload the built packages ===

    let target_type = if matches!(&target, UrlOrPath::Url(_)) {
        "channel"
    } else {
        "directory"
    };
    let target_str = target.to_string();

    pixi_progress::println!(
        "\n{}Publishing {} package(s) to {} {}",
        console::style(console::Emoji("📦 ", "")).cyan(),
        built_package_paths.len(),
        target_type,
        target_str,
    );

    match &target {
        UrlOrPath::Url(url) => {
            pixi_progress::await_in_progress("uploading packages", |_| {
                upload_packages_to_channel(url, &built_package_paths, &ctx)
            })
            .await?;
        }
        UrlOrPath::Path(destination) => {
            upload_to_local_filesystem_path(&built_package_paths, destination, &ctx).await?
        }
    }

    pixi_progress::println!(
        "{}Successfully published {} package(s) to {} {}",
        console::style(console::Emoji("✔ ", "")).green(),
        built_package_paths.len(),
        target_type,
        target_str,
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

/// Parse a target URL string, treating bare paths as `file://` URLs resolved against `base`.
///
/// Single-character schemes (Windows drive letters like `C:`) are treated as paths.
fn parse_target(to: &str, base: &Path) -> miette::Result<Url> {
    if let Ok(url) = Url::parse(to)
        && url.scheme().len() > 1
    {
        return Ok(url);
    }

    let path = Path::new(to);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    Url::from_file_path(&abs).map_err(|()| miette::miette!("'{}' is not a valid path or URL", to))
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
async fn upload_packages_to_channel(
    url: &Url,
    package_paths: &[PathBuf],
    ctx: &PublishContext,
) -> miette::Result<()> {
    let scheme = url.scheme();

    match scheme {
        "s3" => upload_to_s3(url, package_paths, ctx).await,
        "quetz" => upload_to_quetz(url, package_paths, ctx).await,
        "artifactory" => upload_to_artifactory(url, package_paths, ctx).await,
        "prefix" => upload_to_prefix(url, package_paths, ctx).await,
        "file" => {
            let destination = url
                .to_file_path()
                .map_err(|()| miette::miette!("Invalid file URL: {}", url))?;
            upload_to_local_filesystem_channel(&destination, package_paths, ctx).await
        }
        "http" | "https" => {
            let host = url.host_str().unwrap_or("");

            if host.contains("prefix.dev") {
                upload_to_prefix(url, package_paths, ctx).await
            } else if host.contains("anaconda.org") {
                upload_to_anaconda(url, package_paths, ctx).await
            } else if host.contains("quetz") {
                upload_to_quetz(url, package_paths, ctx).await
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

/// Copy packages into a local directory without creating a channel structure.
async fn upload_to_local_filesystem_path(
    package_paths: &[PathBuf],
    destination: &Path,
    ctx: &PublishContext,
) -> miette::Result<()> {
    tokio_fs::create_dir_all(destination)
        .await
        .into_diagnostic()
        .context(format!(
            "Failed to create output directory '{}'",
            destination.display()
        ))?;

    for p in package_paths {
        let file_name = p
            .file_name()
            .ok_or_else(|| miette::miette!("Package path '{}' has no filename", p.display()))?;
        let dest = destination.join(file_name);

        if should_skip_existing(&dest, &file_name.to_string_lossy(), ctx)? {
            continue;
        }

        tokio_fs::copy(p, &dest)
            .await
            .into_diagnostic()
            .context(format!(
                "Failed to copy '{}' to '{}'",
                p.display(),
                dest.display()
            ))?;
    }

    Ok(())
}

/// Decide what to do when a destination path already exists.
///
/// Returns `Ok(true)` when the caller should skip writing (path exists and
/// `skip_existing` is set), `Ok(false)` when the caller should proceed
/// (path is free, or `force` is set), and `Err` when neither flag permits
/// overwriting an existing file.
fn should_skip_existing(
    dest: &Path,
    display_name: &str,
    ctx: &PublishContext,
) -> miette::Result<bool> {
    if !dest.exists() {
        return Ok(false);
    }
    if ctx.skip_existing {
        pixi_progress::println!(
            "{}Skipping '{}' (already exists)",
            console::style(console::Emoji("⏭ ", "")).yellow(),
            display_name
        );
        return Ok(true);
    }
    if !ctx.force {
        return Err(miette::miette!(
            "Package already exists at {}. Use --force to overwrite.",
            dest.display()
        ));
    }
    Ok(false)
}

/// Upload packages to a Prefix.dev server.
async fn upload_to_prefix(
    url: &Url,
    package_paths: &[PathBuf],
    ctx: &PublishContext,
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

    let attestation = if ctx.generate_attestation {
        AttestationSource::GenerateAttestation
    } else {
        AttestationSource::NoAttestation
    };

    let prefix_data = PrefixData::new(
        server_url,
        channel,
        None,
        attestation,
        SkipExisting(ctx.skip_existing),
        ForceOverwrite(ctx.force),
        false,
    );

    upload_package_to_prefix(&ctx.auth_storage, &package_paths.to_vec(), prefix_data)
        .await
        .into_diagnostic()
}

/// Upload packages to Anaconda.org.
async fn upload_to_anaconda(
    url: &Url,
    package_paths: &[PathBuf],
    ctx: &PublishContext,
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
        ForceOverwrite(ctx.force),
    );

    upload_package_to_anaconda(&ctx.auth_storage, &package_paths.to_vec(), anaconda_data)
        .await
        .into_diagnostic()
}

/// Upload packages to a Quetz server.
async fn upload_to_quetz(
    url: &Url,
    package_paths: &[PathBuf],
    ctx: &PublishContext,
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

    upload_package_to_quetz(&ctx.auth_storage, &package_paths.to_vec(), quetz_data).await
}

/// Upload packages to an Artifactory server.
async fn upload_to_artifactory(
    url: &Url,
    package_paths: &[PathBuf],
    ctx: &PublishContext,
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

    upload_package_to_artifactory(&ctx.auth_storage, &package_paths.to_vec(), artifactory_data)
        .await
}

/// Upload packages to S3 and run indexing.
async fn upload_to_s3(
    url: &Url,
    package_paths: &[PathBuf],
    ctx: &PublishContext,
) -> miette::Result<()> {
    use rattler_index::{IndexS3Config, ensure_channel_initialized_s3, index_s3};
    use rattler_upload::upload::upload_package_to_s3;
    use std::collections::HashSet;

    tracing::info!("Uploading packages to S3: {}", url);

    let bucket_config = url.host_str().and_then(|bucket| ctx.s3_options.get(bucket));

    let resolved_credentials = match bucket_config {
        Some(s3_middleware::S3Config::Custom {
            endpoint_url,
            region,
            force_path_style,
        }) => {
            // The workspace manifest or a config file pinned this bucket to a
            // specific endpoint; honor it and pull the access keys from the
            // auth store (populated via `pixi auth login s3://<bucket>`).
            let s3_creds = rattler_s3::S3Credentials {
                endpoint_url: endpoint_url.clone(),
                region: region.clone(),
                addressing_style: if *force_path_style {
                    rattler_s3::S3AddressingStyle::Path
                } else {
                    rattler_s3::S3AddressingStyle::VirtualHost
                },
                access_key_id: None,
                secret_access_key: None,
                session_token: None,
            };
            s3_creds.resolve(url, &ctx.auth_storage).ok_or_else(|| {
                let bucket = url.host_str().unwrap_or("<unknown>");
                miette::miette!(
                    "Bucket '{bucket}' is configured in `s3-options` but no \
                         credentials were found in the auth store. Run \
                         `pixi auth login s3://{bucket}` to store credentials."
                )
            })?
        }
        Some(s3_middleware::S3Config::FromAWS) | None => {
            rattler_s3::ResolvedS3Credentials::from_sdk()
                .await
                .map_err(|e| miette::miette!("Failed to resolve S3 credentials: {}", e))?
        }
    };

    ensure_channel_initialized_s3(url, &resolved_credentials)
        .await
        .map_err(|e| miette::miette!("Failed to initialize S3 channel: {}", e))?;

    let mut subdirs = HashSet::new();
    for package_path in package_paths {
        let subdir = determine_package_subdir(package_path)?;
        subdirs.insert(subdir);
    }

    upload_package_to_s3(
        url.clone(),
        resolved_credentials.clone(),
        &package_paths.to_vec(),
        ctx.force,
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
            repodata_revisions: vec![],
            package_revision_assignment: Default::default(),
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
async fn upload_to_local_filesystem_channel(
    target_dir: &std::path::Path,
    package_paths: &[PathBuf],
    ctx: &PublishContext,
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

        if should_skip_existing(&target_path, &package_name.to_string_lossy(), ctx)? {
            continue;
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
            repodata_revisions: vec![],
            package_revision_assignment: Default::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_s3_options_override_config_for_same_bucket() {
        let mut config = HashMap::new();
        config.insert("bucket-a".to_string(), s3_middleware::S3Config::FromAWS);
        config.insert(
            "bucket-b".to_string(),
            s3_middleware::S3Config::Custom {
                endpoint_url: Url::parse("https://from-config.example/").unwrap(),
                region: "us-east-1".to_string(),
                force_path_style: false,
            },
        );

        let mut manifest = HashMap::new();
        manifest.insert(
            "bucket-b".to_string(),
            S3Options {
                endpoint_url: Url::parse("https://from-manifest.example/").unwrap(),
                region: "eu-central-1".to_string(),
                force_path_style: true,
            },
        );
        manifest.insert(
            "bucket-c".to_string(),
            S3Options {
                endpoint_url: Url::parse("https://only-in-manifest.example/").unwrap(),
                region: "ap-south-1".to_string(),
                force_path_style: false,
            },
        );

        let merged = merge_s3_options(config, Some(&manifest));

        assert!(matches!(
            merged.get("bucket-a"),
            Some(s3_middleware::S3Config::FromAWS),
        ));
        let s3_middleware::S3Config::Custom {
            endpoint_url,
            region,
            force_path_style,
        } = merged.get("bucket-b").unwrap()
        else {
            panic!("bucket-b should resolve to a Custom config from the manifest");
        };
        assert_eq!(endpoint_url.as_str(), "https://from-manifest.example/");
        assert_eq!(region, "eu-central-1");
        assert!(force_path_style);
        assert!(merged.contains_key("bucket-c"));
    }

    #[test]
    fn merge_s3_options_passes_base_through_when_manifest_absent() {
        let mut config = HashMap::new();
        config.insert("bucket-a".to_string(), s3_middleware::S3Config::FromAWS);

        let merged = merge_s3_options(config.clone(), None);
        assert_eq!(merged.len(), config.len());
        assert!(matches!(
            merged.get("bucket-a"),
            Some(s3_middleware::S3Config::FromAWS),
        ));
    }
}
