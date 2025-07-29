use std::path::PathBuf;

use clap::Parser;
use indicatif::ProgressBar;
use miette::{Context, IntoDiagnostic};
use pixi_command_dispatcher::{
    BuildBackendMetadataSpec, BuildEnvironment, BuildProfile, CacheDirs, SourceBuildSpec,
};
use pixi_config::ConfigCli;
use pixi_manifest::FeaturesExt;
use pixi_progress::global_multi_progress;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec};
use rattler_conda_types::{GenericVirtualPackage, Platform};

use crate::{WorkspaceLocator, cli::cli_config::WorkspaceConfig};
use pixi_reporters::TopLevelProgress;

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
        .with_closest_package(true)
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
        .virtual_packages(Platform::current())
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
        build_platform: Platform::current(),
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

    // Determine the source of the package - use src from build config if available,
    // otherwise use current directory
    let source: PinnedSourceSpec = if let Some(package) = &workspace.package {
        if let Some(src_spec) = &package.value.build.src {
            match src_spec {
                pixi_spec::SourceSpec::Path(path_spec) => {
                    let resolved_path = path_spec.resolve(search_start).into_diagnostic()?;
                    PinnedPathSpec {
                        path: resolved_path.to_string_lossy().into_owned().into(),
                    }
                    .into()
                }
                _ => {
                    miette::bail!(
                        "Url and git source roots are not yet supported. Use path-based source roots for now."
                    )
                }
            }
        } else {
            PinnedPathSpec {
                path: search_start.to_string_lossy().into_owned().into(),
            }
            .into()
        }
    } else {
        PinnedPathSpec {
            path: search_start.to_string_lossy().into_owned().into(),
        }
        .into()
    };

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

    // Ensure the output directory exists
    fs_err::create_dir_all(&args.output_dir)
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to create output directory '{0}'",
                args.output_dir.display()
            )
        })?;

    // Build the individual packages
    for package in packages {
        let built_package = command_dispatcher
            .source_build(SourceBuildSpec {
                package,
                output_directory: Some(args.output_dir.clone()),
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

        // Canonicalize the output directory and package path.
        let output_dir = dunce::canonicalize(&args.output_dir)
            .expect("failed to canonicalize output directory which must now exist");
        let package_path = dunce::canonicalize(&built_package.output_file)
            .expect("failed to canonicalize output file which must now exist");

        // Make the path relative to the output directory
        let output_file = pathdiff::diff_paths(&package_path, &output_dir)
            .map(|p| args.output_dir.join(p))
            .unwrap_or_else(|| dunce::simplified(&built_package.output_file).to_path_buf());
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
