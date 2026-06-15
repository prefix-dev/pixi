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
    ComputeResultExt, CondaPackageFormat, EnvironmentRef, EnvironmentSpec, EphemeralEnv,
    keys::{ResolveSourcePackageKey, ResolveSourcePackageSpec, SourceBuildKey, SourceBuildSpec},
};
use pixi_config::{ConfigCli, PackageFormatAndCompression};
use pixi_core::{
    Workspace, WorkspaceLocator, environment::sanity_check_workspace, workspace::DiscoveryStart,
};
use pixi_manifest::{FeaturesExt, S3Options};
use pixi_path::AbsPathBuf;
use pixi_progress::global_multi_progress;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec};
use pixi_reporters::TopLevelProgress;
use pixi_spec::SourceLocationSpec;
use pixi_utils::variants::{VariantConfig, VariantValue};
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_networking::{AuthenticationStorage, s3_middleware};
use rattler_package_streaming::seek::read_package_file;

/// Build a conda package and publish it to a channel.
///
/// Builds the package from your workspace and either uploads it to a channel
/// (`--target-channel`) or copies the artifact into a local directory
/// (`--target-dir`).
///
/// Supported destinations for `--target-channel` (alias `--to`):
///   - prefix.dev: `https://prefix.dev/<channel-name>`
///   - anaconda.org: `https://anaconda.org/<owner>/<label>`
///   - Cloudsmith: `cloudsmith://<owner>/<repository>`
///   - S3: `s3://bucket-name`
///   - Quetz: `quetz://server/<channel>`
///   - Artifactory: `artifactory://server/<channel>`
///   - Local filesystem channel (with indexing):
///     `file:///path/to/channel` or a bare path
///
/// Use `--target-dir <PATH>` instead to copy the built package(s) into a
/// directory without creating a channel structure.
#[derive(Parser, Debug)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

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

    /// The target channel to publish packages to. Accepts a URL (prefix.dev, anaconda.org, cloudsmith://, s3://, quetz://, artifactory://) or a local filesystem path / `file://` URL for an indexed local channel.
    ///
    /// Mutually exclusive with `--target-dir`.
    #[arg(long, visible_alias = "to", conflicts_with = "target_dir")]
    pub target_channel: Option<String>,

    /// The local filesystem path to copy the built package(s) into (no channel indexing).
    ///
    /// Accepts an absolute or relative directory path. Mutually exclusive with `--target-channel`.
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

    /// Override a build variant key with one or more values.
    ///
    /// Use `--variant KEY=VALUE` to build only that variant, or
    /// `--variant KEY=VAL1,VAL2,...` to constrain a variant to a subset of
    /// values. Repeat the flag for multiple keys. Values supplied here
    /// replace any matching key from the workspace `build-variants` and
    /// any `--variant-config` files.
    ///
    /// Example: `pixi publish --variant python=3.12 --variant cuda-version=12.8,13.0`.
    #[arg(long = "variant", value_parser = parse_variant, value_name = "KEY=VALUES")]
    pub variant: Vec<(String, Vec<String>)>,

    /// Path to an additional variant configuration YAML file.
    ///
    /// Mirrors rattler-build's `--variant-config/-m`. Repeat to add
    /// multiple files. Paths are appended after the workspace-level
    /// `build-variants-files`.
    #[arg(long = "variant-config", short = 'm', value_name = "FILE")]
    pub variant_config: Vec<PathBuf>,

    /// Archive format and optional compression level, e.g. `conda`,
    /// `tar-bz2`, `conda:max`, `conda:15`, `tar-bz2:9`. Numeric ranges
    /// match rattler-build: -7..=22 for `.conda`, 1..=9 for `.tar.bz2`.
    #[arg(long)]
    pub package_format: Option<PackageFormatAndCompression>,
}

/// Parse a `KEY=VAL[,VAL...]` variant override.
pub(crate) fn parse_variant(
    s: &str,
) -> Result<(String, Vec<String>), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=VALUE: no `=` found in `{s}`"))?;
    let key = s[..pos].trim().to_string();
    if key.is_empty() {
        return Err(format!("invalid KEY=VALUE: empty key in `{s}`").into());
    }
    let raw_value = &s[pos + 1..];
    if raw_value.contains('=') {
        return Err(format!(
            "invalid KEY=VALUE: value in `{s}` contains an `=`; values cannot contain `=`",
        )
        .into());
    }
    let values: Vec<String> = raw_value
        .split(',')
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect();
    if values.is_empty() {
        return Err(format!("invalid KEY=VALUE: empty value in `{s}`").into());
    }
    Ok((key, values))
}

/// Build a `BTreeMap` from CLI `--variant` overrides. Repeated flags with the
/// same key accumulate values.
fn cli_variants_map(cli: &[(String, Vec<String>)]) -> BTreeMap<String, Vec<VariantValue>> {
    let mut grouped: BTreeMap<String, Vec<VariantValue>> = BTreeMap::new();
    for (key, values) in cli {
        grouped
            .entry(key.clone())
            .or_default()
            .extend(values.iter().cloned().map(VariantValue::from));
    }
    grouped
}

/// Variant keys whose values either differ across the built outputs or were
/// explicitly overridden on the CLI. These are the ones worth surfacing in
/// per-package summaries — printing every keyed variant for every package would
/// be noisy when most of them are identical across outputs.
fn distinguishing_variant_keys(
    package_variants: &[&BTreeMap<String, VariantValue>],
    cli_keys: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut all_keys: BTreeSet<String> = BTreeSet::new();
    for variants in package_variants {
        all_keys.extend(variants.keys().cloned());
    }

    let mut keys = BTreeSet::new();
    for key in all_keys {
        if cli_keys.contains(&key) {
            keys.insert(key);
            continue;
        }
        let distinct: BTreeSet<Option<&VariantValue>> =
            package_variants.iter().map(|v| v.get(&key)).collect();
        if distinct.len() > 1 {
            keys.insert(key);
        }
    }
    keys
}

/// Render the selected variants for a single package as ` (key: value, ...)`,
/// or an empty string when there are none to show.
fn format_variant_suffix(
    variants: &BTreeMap<String, VariantValue>,
    keys: &BTreeSet<String>,
) -> String {
    let parts: Vec<String> = keys
        .iter()
        .filter_map(|k| variants.get(k).map(|v| format!("{k}: {v}")))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

/// Collect every `--variant KEY=VAL` pair that doesn't appear in any of the
/// resolved outputs. Catches typos like `--variant cmke=4.3.0` and flags values
/// that were dropped because they had no matching variant.
///
/// Returns `(unused_keys, unused_values)` where each `unused_values` entry is
/// `(key, value)`.
fn unused_cli_variants(
    cli_variants: &[(String, Vec<String>)],
    package_variants: &[&BTreeMap<String, VariantValue>],
) -> (Vec<String>, Vec<(String, String)>) {
    let mut used: BTreeMap<&String, BTreeSet<&VariantValue>> = BTreeMap::new();
    for variants in package_variants {
        for (k, v) in *variants {
            used.entry(k).or_default().insert(v);
        }
    }

    let mut unused_keys = Vec::new();
    let mut unused_values = Vec::new();
    for (key, values) in cli_variants {
        match used.get(key) {
            None => unused_keys.push(key.clone()),
            Some(used_values) => {
                for value in values {
                    let candidate = VariantValue::from(value.as_str());
                    if !used_values.iter().any(|v| **v == candidate) {
                        unused_values.push((key.clone(), value.clone()));
                    }
                }
            }
        }
    }
    (unused_keys, unused_values)
}

/// Print warnings for `--variant` overrides that didn't make it into any
/// built output. Wraps [`unused_cli_variants`] with the user-facing format.
fn warn_unused_cli_variants(
    cli_variants: &[(String, Vec<String>)],
    package_variants: &[&BTreeMap<String, VariantValue>],
) {
    let (unused_keys, unused_values) = unused_cli_variants(cli_variants, package_variants);
    let warn_prefix = console::style(console::Emoji("⚠️  ", "warning: ")).yellow();
    for key in unused_keys {
        pixi_progress::println!(
            "{}variant key '{}' was not used by any built package; check for typos",
            warn_prefix,
            key,
        );
    }
    for (key, value) in unused_values {
        pixi_progress::println!(
            "{}variant value '{}={}' was not used by any built package",
            warn_prefix,
            key,
            value,
        );
    }
}

/// Resolve CLI `--variant-config` paths against the given working directory.
fn resolve_variant_config_paths(paths: &[PathBuf], cwd: &Path) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            }
        })
        .collect()
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
    /// quetz, artifactory, cloudsmith) and as the access-key source when S3 credentials
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
        .with_global_config_source(args.config_source.source())
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

    let target_pixi_platform = pixi_manifest::PixiPlatform::from_subdir(args.target_platform);
    let build_pixi_platform = pixi_manifest::PixiPlatform::from_subdir(args.build_platform);
    let VariantConfig {
        mut variant_configuration,
        mut variant_files,
    } = workspace.variants(&target_pixi_platform)?;

    // Overlay CLI `--variant KEY=VAL[,VAL...]` overrides on top of the workspace
    // variants. Multiple `--variant` flags with the same key accumulate values,
    // but as a group they replace any matching key from the workspace.
    variant_configuration.extend(cli_variants_map(&args.variant));

    // Append CLI-provided variant config files. Relative paths are resolved
    // against the current working directory so callers can pass `-m variants.yaml`
    // from wherever they invoke pixi.
    if !args.variant_config.is_empty() {
        let cwd = std::env::current_dir().into_diagnostic()?;
        variant_files.extend(resolve_variant_config_paths(&args.variant_config, &cwd));
    }

    let build_virtual_packages: Vec<GenericVirtualPackage> = workspace
        .default_environment()
        .virtual_packages(&build_pixi_platform)
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();

    let host_virtual_packages: Vec<GenericVirtualPackage> = workspace
        .default_environment()
        .virtual_packages(&target_pixi_platform)
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

    // The CondaOutput metadata uses `pixi_build_types::VariantValue`, while the
    // rest of the publish flow (and our helpers) work with the
    // `pixi_utils::variants::VariantValue` re-export. Convert once at the
    // boundary so the helpers see a uniform value type.
    let pkg_variant_maps_owned: Vec<BTreeMap<String, VariantValue>> = packages
        .iter()
        .map(|p| {
            p.metadata
                .variant
                .iter()
                .map(|(k, v)| (k.clone(), VariantValue::from(v.clone())))
                .collect()
        })
        .collect();
    let pkg_variant_maps: Vec<&BTreeMap<String, VariantValue>> =
        pkg_variant_maps_owned.iter().collect();

    // Surface `--variant KEY=VAL` overrides that didn't make it into any output
    // (typically typos like `cmke=4.3.0`) before the build runs.
    warn_unused_cli_variants(&args.variant, &pkg_variant_maps);

    let cli_variant_keys: BTreeSet<String> = args.variant.iter().map(|(k, _)| k.clone()).collect();
    let display_variant_keys = distinguishing_variant_keys(&pkg_variant_maps, &cli_variant_keys);

    // Print initial build summary
    pixi_progress::println!(
        "\n{}Building {} package(s):",
        console::style(console::Emoji("📋 ", "")).cyan(),
        packages.len()
    );
    for (pkg, variants) in packages.iter().zip(&pkg_variant_maps_owned) {
        pixi_progress::println!(
            "  - {} v{} [{}] ({}){}",
            pkg.metadata.name.as_normalized(),
            pkg.metadata.version,
            pkg.metadata.build,
            pkg.metadata.subdir,
            format_variant_suffix(variants, &display_variant_keys),
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

    // Build and collect all package paths along with the variants each was
    // built with, so the publish summary can attribute every artifact back to
    // the variant that produced it.
    let mut built_packages: Vec<(PathBuf, BTreeMap<String, VariantValue>)> = Vec::new();

    for record in resolved_records {
        let record = Arc::unwrap_or_clone(record);
        let variants = record.variants.clone();
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
            package_format: args.package_format.as_ref().map(|f| CondaPackageFormat {
                archive_type: f.archive_type,
                compression_level: f.compression_level.into(),
            }),
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

        built_packages.push((package_path, variants));
    }

    // Drop the dispatcher (and its repodata gateway) before indexing. The
    // gateway memory-maps the target channel's `repodata.json`; on Windows that
    // mapping blocks the indexer from overwriting it (os error 1224). See #6362.
    drop(command_dispatcher);

    if built_packages.is_empty() {
        miette::bail!("No packages were built. Nothing to publish.");
    }

    let built_package_paths: Vec<PathBuf> = built_packages.iter().map(|(p, _)| p.clone()).collect();

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
    for (path, variants) in &built_packages {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        pixi_progress::println!(
            "  - {}{}",
            name,
            format_variant_suffix(variants, &display_variant_keys),
        );
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
        "cloudsmith" => upload_to_cloudsmith(url, package_paths, ctx).await,
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
                    Supported hosts: prefix.dev, anaconda.org, or use explicit schemes: s3://, quetz://, artifactory://, prefix://, cloudsmith://",
                    url
                ))
            }
        }
        _ => Err(miette::miette!(
            "Unsupported URL scheme '{}'. Supported schemes: file://, s3://, quetz://, artifactory://, prefix://, cloudsmith://, http://, https://",
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

/// Upload packages to Cloudsmith.
async fn upload_to_cloudsmith(
    url: &Url,
    package_paths: &[PathBuf],
    ctx: &PublishContext,
) -> miette::Result<()> {
    use rattler_upload::upload::opt::CloudsmithData;
    use rattler_upload::upload::upload_package_to_cloudsmith;

    tracing::info!("Uploading packages to Cloudsmith: {}", url);

    let owner = url
        .host_str()
        .ok_or_else(|| miette::miette!("Invalid Cloudsmith URL: missing owner"))?
        .to_string();

    let mut segments = url
        .path_segments()
        .ok_or_else(|| miette::miette!("Invalid Cloudsmith URL: missing repo"))?
        .filter(|s| !s.is_empty());

    let repo = segments
        .next()
        .ok_or_else(|| miette::miette!("Invalid Cloudsmith URL: missing repo"))?
        .to_string();

    if segments.next().is_some() {
        return Err(miette::miette!(
            "Invalid Cloudsmith URL: expected cloudsmith://owner/repo"
        ));
    }

    let api_key = std::env::var("CLOUDSMITH_API_KEY").ok();
    let api_url = std::env::var("CLOUDSMITH_API_URL")
        .ok()
        .map(|url| url.parse())
        .transpose()
        .into_diagnostic()
        .context("Failed to parse CLOUDSMITH_API_URL")?;
    let cloudsmith_data = CloudsmithData::new(owner, repo, api_key, api_url);

    upload_package_to_cloudsmith(&ctx.auth_storage, &package_paths.to_vec(), cloudsmith_data)
        .await
        .into_diagnostic()
        .context("Failed to upload packages to Cloudsmith")
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
    use std::str::FromStr;

    use super::*;
    use rattler_conda_types::{compression_level::CompressionLevel, package::CondaArchiveType};

    #[test]
    fn parse_variant_accepts_single_and_comma_list() {
        assert_eq!(
            parse_variant("python=3.12").unwrap(),
            ("python".into(), vec!["3.12".into()]),
        );
        assert_eq!(
            parse_variant("cuda-version=12.8,13.0").unwrap(),
            ("cuda-version".into(), vec!["12.8".into(), "13.0".into()]),
        );
    }

    #[test]
    fn parses_bare_package_format() {
        let parsed = PackageFormatAndCompression::from_str("conda").unwrap();
        assert_eq!(parsed.archive_type, CondaArchiveType::Conda);
        assert_eq!(parsed.compression_level, CompressionLevel::Default);
    }

    #[test]
    fn parses_named_compression_level() {
        let parsed = PackageFormatAndCompression::from_str("conda:max").unwrap();
        assert_eq!(parsed.archive_type, CondaArchiveType::Conda);
        assert_eq!(parsed.compression_level, CompressionLevel::Highest);
    }

    #[test]
    fn parses_numeric_compression_level() {
        let parsed = PackageFormatAndCompression::from_str("tar-bz2:5").unwrap();
        assert_eq!(parsed.archive_type, CondaArchiveType::TarBz2);
        assert_eq!(parsed.compression_level, CompressionLevel::Numeric(5));
    }

    #[test]
    fn rejects_unknown_format() {
        assert!(PackageFormatAndCompression::from_str("zip").is_err());
    }

    #[test]
    fn rejects_out_of_range_numeric_level() {
        assert!(PackageFormatAndCompression::from_str("tar-bz2:42").is_err());
        assert!(PackageFormatAndCompression::from_str("conda:99").is_err());
    }

    #[test]
    fn parse_variant_rejects_malformed_input() {
        for s in ["python", "=3.12", "python=", "python=,,", "expr=a=b"] {
            assert!(parse_variant(s).is_err(), "expected error for {s:?}");
        }
    }

    #[test]
    fn cli_variants_map_replaces_workspace_key_and_accumulates_repeats() {
        let mut variants = BTreeMap::from([
            (
                "python".into(),
                vec!["3.10".into(), "3.11".into(), "3.12".into()],
            ),
            ("cuda-version".into(), vec!["12.8".into(), "13.0".into()]),
        ]);
        variants.extend(cli_variants_map(&[
            ("python".into(), vec!["3.11".into()]),
            ("python".into(), vec!["3.12".into()]),
        ]));

        assert_eq!(
            variants.get("python").unwrap(),
            &vec![VariantValue::from("3.11"), VariantValue::from("3.12")],
        );
        // Workspace keys not mentioned by the CLI must be left untouched.
        assert_eq!(
            variants.get("cuda-version").unwrap(),
            &vec![VariantValue::from("12.8"), VariantValue::from("13.0")],
        );
    }

    fn variant_map<const N: usize>(entries: [(&str, &str); N]) -> BTreeMap<String, VariantValue> {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), VariantValue::from(v)))
            .collect()
    }

    #[test]
    fn distinguishing_variant_keys_picks_differing_and_cli_keys() {
        let pkg_a = variant_map([("python", "3.12"), ("cmake", "4.3.0")]);
        let pkg_b = variant_map([("python", "3.12"), ("cmake", "4.3.2")]);
        let pkgs = vec![&pkg_a, &pkg_b];

        // Only `cmake` differs across packages.
        assert_eq!(
            distinguishing_variant_keys(&pkgs, &BTreeSet::new()),
            BTreeSet::from(["cmake".to_string()]),
        );

        // CLI-overridden keys are surfaced even when they don't differ.
        let cli_keys = BTreeSet::from(["python".to_string()]);
        assert_eq!(
            distinguishing_variant_keys(&pkgs, &cli_keys),
            BTreeSet::from(["cmake".to_string(), "python".to_string()]),
        );
    }

    #[test]
    fn format_variant_suffix_renders_selected_keys() {
        let variants = variant_map([("cmake", "4.3.0"), ("python", "3.12")]);
        let keys = BTreeSet::from(["cmake".to_string(), "python".to_string()]);
        assert_eq!(
            format_variant_suffix(&variants, &keys),
            " (cmake: 4.3.0, python: 3.12)",
        );

        // Single-key case.
        let only_cmake = BTreeSet::from(["cmake".to_string()]);
        assert_eq!(
            format_variant_suffix(&variants, &only_cmake),
            " (cmake: 4.3.0)",
        );

        // No selected keys → empty string (no trailing space).
        assert_eq!(format_variant_suffix(&variants, &BTreeSet::new()), "");

        // Selected key missing from this package → skipped.
        let absent = BTreeSet::from(["cuda".to_string()]);
        assert_eq!(format_variant_suffix(&variants, &absent), "");
    }

    #[test]
    fn unused_cli_variants_flags_typos_and_dropped_values() {
        let pkg_a = variant_map([("cmake", "4.3.0")]);
        let pkg_b = variant_map([("cmake", "4.3.2")]);
        let pkgs = vec![&pkg_a, &pkg_b];

        let cli = vec![
            // Typo: never appears in any output.
            ("cmke".to_string(), vec!["4.3.0".to_string()]),
            // Mixed: 4.3.0 and 4.3.2 are used, 5.0.0 is not.
            (
                "cmake".to_string(),
                vec![
                    "4.3.0".to_string(),
                    "4.3.2".to_string(),
                    "5.0.0".to_string(),
                ],
            ),
        ];
        let (unused_keys, unused_values) = unused_cli_variants(&cli, &pkgs);
        assert_eq!(unused_keys, vec!["cmke".to_string()]);
        assert_eq!(
            unused_values,
            vec![("cmake".to_string(), "5.0.0".to_string())]
        );
    }

    #[test]
    fn unused_cli_variants_silent_when_everything_used() {
        let pkg = variant_map([("python", "3.12")]);
        let pkgs = vec![&pkg];
        let cli = vec![("python".to_string(), vec!["3.12".to_string()])];
        let (keys, values) = unused_cli_variants(&cli, &pkgs);
        assert!(keys.is_empty());
        assert!(values.is_empty());
    }

    #[test]
    fn resolve_variant_config_paths_anchors_relatives_to_cwd() {
        let cwd = Path::new("/work/repo");
        let resolved = resolve_variant_config_paths(
            &[
                PathBuf::from("variants.yaml"),
                PathBuf::from("ci/variants.yaml"),
                PathBuf::from("/abs/variants.yaml"),
            ],
            cwd,
        );
        assert_eq!(
            resolved,
            vec![
                PathBuf::from("/work/repo/variants.yaml"),
                PathBuf::from("/work/repo/ci/variants.yaml"),
                PathBuf::from("/abs/variants.yaml"),
            ],
        );
    }

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
