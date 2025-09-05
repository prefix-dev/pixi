use std::path::PathBuf;

use clap::Parser;
use indicatif::ProgressBar;
use miette::{Context, IntoDiagnostic};
use pixi_command_dispatcher::{
    BuildBackendMetadataSpec, BuildEnvironment, BuildProfile, CacheDirs, SourceBuildSpec,
};
use pixi_config::ConfigCli;
use pixi_core::WorkspaceLocator;
use pixi_manifest::FeaturesExt;
use pixi_progress::global_multi_progress;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec};
use pixi_reporters::TopLevelProgress;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use tempfile::tempdir;

use crate::cli_config::WorkspaceConfig;

#[derive(Parser, Debug)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: WorkspaceConfig,

    #[clap(flatten)]
    pub config_cli: ConfigCli,

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
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Locate the workspace based on the provided configuration.
    let workspace_locator = args.project_config.workspace_locator_start();
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
    let variant_configuration = workspace.variants(args.target_platform);

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
    let Ok(search_start) = workspace_locator.path() else {
        miette::bail!("could not determine the current working directory to locate the workspace");
    };
    let channel_config = workspace.channel_config();
    let channels = workspace
        .default_environment()
        .channel_urls(&channel_config)
        .into_diagnostic()?;

    // Determine the source of the package.
    let source: PinnedSourceSpec = PinnedPathSpec {
        path: search_start.to_string_lossy().into_owned().into(),
    }
    .into();

    // Create the build backend metadata specification.
    let backend_metadata_spec = BuildBackendMetadataSpec {
        source: source.clone(),
        channels: channels.clone(),
        channel_config: channel_config.clone(),
        build_environment: build_environment.clone(),
        variants: Some(variant_configuration.clone()),
        enabled_protocols: Default::default(),
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
                source: source.clone(),
                channels: channels.clone(),
                channel_config: channel_config.clone(),
                build_environment: build_environment.clone(),
                variants: Some(variant_configuration.clone()),
                enabled_protocols: Default::default(),
                work_directory: None,
                clean: args.clean,
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
