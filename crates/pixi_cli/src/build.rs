use std::{ffi::OsStr, path::PathBuf};

use clap::Parser;
use fs_err::tokio as tokio_fs;
use indicatif::ProgressBar;
use miette::{Context, IntoDiagnostic};
use pixi_command_dispatcher::{
    BuildBackendMetadataSpec, BuildEnvironment, BuildProfile, CacheDirs, SourceBuildSpec,
};
use pixi_config::ConfigCli;
use pixi_consts::consts::{
    MOJOPROJECT_MANIFEST, PYPROJECT_MANIFEST, RATTLER_BUILD_FILE_NAMES, ROS_BACKEND_FILE_NAMES,
    WORKSPACE_MANIFEST,
};
use pixi_core::{WorkspaceLocator, workspace::DiscoveryStart};
use pixi_manifest::FeaturesExt;
use pixi_progress::global_multi_progress;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec};
use pixi_reporters::TopLevelProgress;
use pixi_utils::variants::VariantConfig;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use tempfile::tempdir;

use crate::cli_config::LockAndInstallConfig;

#[derive(Parser, Debug)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub config_cli: ConfigCli,

    #[clap(flatten)]
    pub lock_and_install_config: LockAndInstallConfig,

    /// The target platform to build for (defaults to the current platform)
    #[clap(long, short, default_value_t = Platform::current())]
    pub target_platform: Platform,

    /// The build platform to use for building (defaults to the current platform)
    #[clap(long, default_value_t = Platform::current())]
    pub build_platform: Platform,

    /// The output directory to place the built artifacts
    #[clap(long, short, default_value = ".")]
    pub output_dir: PathBuf,

    /// The directory to use for incremental builds artifacts.
    #[clap(long, short)]
    pub build_dir: Option<PathBuf>,

    /// Whether to clean the build directory before building.
    #[clap(long, short)]
    pub clean: bool,

    /// The path to a directory containing a package manifest, or to a specific manifest file.
    ///
    /// Supported manifest files: `package.xml`, `recipe.yaml`, `pixi.toml`, `pyproject.toml`, or `mojoproject.toml`.
    ///
    /// When a directory is provided, the command will search for supported manifest files within it.
    #[arg(long)]
    pub path: Option<PathBuf>,
}

/// Validate that the full path of package manifest exists and is a supported format.
/// Directories are allowed (for discovery), and specific manifest files must be supported formats.
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

pub(crate) async fn determine_discovery_start(
    path: &Option<PathBuf>,
) -> miette::Result<DiscoveryStart> {
    match path {
        Some(path) => {
            // // Validate the path first
            // validate_package_manifest(path).await?;

            // If it's a directory, use it as the search root
            if path.is_dir() {
                Ok(DiscoveryStart::SearchRoot(path.clone()))
            } else {
                // If it's a file, use its parent directory as the search root
                let package_dir = path.parent().ok_or_else(|| {
                    miette::miette!("Failed to get parent directory of package manifest")
                })?;
                Ok(DiscoveryStart::SearchRoot(package_dir.to_path_buf()))
            }
        }
        // If no path is provided, use the current directory
        None => Ok(DiscoveryStart::CurrentDir),
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Locate the workspace based on the provided configuration.
    // When --path is specified, we should find the workspace manifest relative
    // to the path's directory, not the current working directory.
    let workspace_locator = determine_discovery_start(&args.path).await?;

    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(workspace_locator.clone())
        .with_closest_package(false)
        .locate()?
        .with_cli_config(args.config_cli);

    // Construct a command dispatcher based on the workspace.
    let multi_progress = global_multi_progress();
    let anchor_pb = multi_progress.add(ProgressBar::hidden());
    let mut cache_dirs =
        CacheDirs::new(pixi_config::get_cache_dir()?).with_workspace(workspace.pixi_dir());
    if let Some(build_dir) = args.build_dir {
        cache_dirs.set_working_dirs(build_dir);
    }
    let command_dispatcher = workspace
        .command_dispatcher_builder()?
        .with_cache_dirs(cache_dirs)
        .with_reporter(TopLevelProgress::new(multi_progress, anchor_pb))
        .finish();

    // Determine the variant configuration for the build.
    let VariantConfig {
        variants,
        variant_files,
    } = workspace.variants(args.target_platform)?;

    // Build platform virtual packages
    let build_virtual_packages: Vec<GenericVirtualPackage> = workspace
        .default_environment()
        .virtual_packages(args.build_platform)
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();

    // Host platform virtual packages
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

    // Query any and all information we can acquire about the package we're
    // attempting to build.
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

    let channel_config = workspace.channel_config();
    let channels = workspace
        .default_environment()
        .channel_urls(&channel_config)
        .into_diagnostic()?;

    let manifest_source: PinnedSourceSpec = PinnedPathSpec {
        path: package_manifest_path_canonical
            .to_string_lossy()
            .into_owned()
            .into(),
    }
    .into();

    // Create the build backend metadata specification.
    let backend_metadata_spec = BuildBackendMetadataSpec {
        manifest_source: manifest_source.clone(),
        channels: channels.clone(),
        channel_config: channel_config.clone(),
        build_environment: build_environment.clone(),
        variants: Some(variants.clone()),
        variant_files: Some(variant_files.clone()),
        enabled_protocols: Default::default(),
        pin_override: None,
    };
    let backend_metadata = command_dispatcher
        .build_backend_metadata(backend_metadata_spec.clone())
        .await?;

    // Determine all the outputs available from the build backend.
    let packages = backend_metadata.metadata.outputs();

    // Ensure the final output directory exists
    fs_err::create_dir_all(&args.output_dir)
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to create output directory '{0}'",
                args.output_dir.display()
            )
        })?;

    // Create a temporary directory to hold build outputs
    let temp_output_dir = tempdir()
        .into_diagnostic()
        .context("failed to create temporary output directory for build artifacts")?;

    // Build the individual packages
    for package in packages {
        let built_package = command_dispatcher
            .source_build(SourceBuildSpec {
                package,
                // Build into a temporary directory first
                output_directory: Some(temp_output_dir.path().to_path_buf()),
                manifest_source: manifest_source.clone(),
                build_source: None,
                channels: channels.clone(),
                channel_config: channel_config.clone(),
                build_environment: build_environment.clone(),
                variants: Some(variants.clone()),
                variant_files: Some(variant_files.clone()),
                enabled_protocols: Default::default(),
                work_directory: None,
                clean: args.clean,
                force: false,
                build_profile: BuildProfile::Release,
            })
            .await?;

        // Clear the top level progress
        command_dispatcher.clear_reporter().await;

        let package_path = dunce::canonicalize(&built_package.output_file)
            .expect("failed to canonicalize output file which must now exist");

        // Destination inside the user-requested output directory
        let file_name = package_path
            .file_name()
            .expect("built package should have a file name");
        let dest_path = args.output_dir.join(file_name);

        // Move the .conda artifact into the requested directory.
        // If a simple rename fails (e.g., across filesystems), fall back to copy+remove.
        if let Err(_e) = fs_err::rename(&package_path, &dest_path) {
            fs_err::copy(&package_path, &dest_path).into_diagnostic()?;
            fs_err::remove_file(&package_path).into_diagnostic()?;
        }

        // Print success relative to the user-requested output directory
        let output_dir = dunce::canonicalize(&args.output_dir)
            .expect("failed to canonicalize output directory which must now exist");
        let dest_canon = dunce::canonicalize(&dest_path)
            .expect("failed to canonicalize moved output file which must now exist");
        let output_file = pathdiff::diff_paths(&dest_canon, &output_dir)
            .map(|p| args.output_dir.join(p))
            .unwrap_or_else(|| dunce::simplified(&dest_path).to_path_buf());
        let stripped_output_file = output_file
            .strip_prefix(&args.output_dir)
            .unwrap_or(&output_file);

        pixi_progress::println!(
            "{}Successfully built '{}'",
            console::style(console::Emoji("✔ ", "")).green(),
            stripped_output_file.display()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_discovery_start_with_file_path() {
        let discovery_start = determine_discovery_start(&Some(PathBuf::from(
            "tests/fixtures/build_tests/recipe/recipe.yaml",
        )))
        .await
        .unwrap();

        let discovery_start_path = discovery_start.path().unwrap();
        let expected_path = PathBuf::from("tests/fixtures/build_tests/recipe");
        assert_eq!(discovery_start_path, expected_path);
    }

    #[tokio::test]
    async fn test_discovery_start_with_directory_path() {
        // Use the current directory which always exists
        let test_dir = PathBuf::from(".");
        let discovery_start = determine_discovery_start(&Some(test_dir.clone()))
            .await
            .unwrap();

        let discovery_start_path = discovery_start.path().unwrap();
        assert_eq!(discovery_start_path, test_dir);
    }

    #[tokio::test]
    async fn test_discovery_start_without_path() {
        let discovery_start = determine_discovery_start(&None).await.unwrap();

        // When no path is provided, it should use CurrentDir
        assert!(matches!(discovery_start, DiscoveryStart::CurrentDir));
    }
}
