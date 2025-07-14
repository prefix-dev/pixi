use std::path::PathBuf;

use clap::Parser;
use indicatif::ProgressBar;
use miette::{Context, IntoDiagnostic};
use pixi_build_discovery::EnabledProtocols;
use pixi_build_types::{
    ChannelConfiguration, PlatformAndVirtualPackages, procedures::conda_build_v0::CondaBuildParams,
};
use pixi_command_dispatcher::{InstantiateBackendSpec, SourceCheckout, build::WorkDirKey};
use pixi_config::ConfigCli;
use pixi_manifest::FeaturesExt;
use pixi_progress::global_multi_progress;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec};
use rattler_conda_types::{GenericVirtualPackage, Platform};
use typed_path::Utf8TypedPath;

use crate::{
    WorkspaceLocator,
    build::BuildContext,
    cli::cli_config::WorkspaceConfig,
    reporters::TopLevelProgress,
    utils::{MoveError, move_file},
};

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

    /// Whether to build incrementally if possible
    #[clap(long, short)]
    pub no_incremental: bool,

    /// The directory to use for incremental builds artifacts
    #[clap(long, short)]
    pub build_dir: Option<PathBuf>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.project_config.workspace_locator_start())
        .with_closest_package(true)
        .locate()?
        .with_cli_config(args.config_cli);

    let Some(package_manifest) = &workspace.package else {
        miette::bail!(
            help = "To build a package in a workspace, execute the command from the directory containing the package.",
            "the pixi workspace at {} does not contain a package manifest",
            workspace.workspace.provenance.path.display()
        );
    };

    // TODO: Implement logic to take the source code from a VCS instead of from a
    // local channel so that that information is also encoded in the manifest.

    // Instantiate a protocol for the source directory.
    let channel_config = workspace.channel_config();

    // Instantiate the command dispatcher.
    let command_dispatcher = workspace
        .command_dispatcher_builder()?
        .with_reporter(TopLevelProgress::new(
            global_multi_progress(),
            global_multi_progress().add(ProgressBar::hidden()),
        ))
        .finish();

    // Instantiate the build backend.
    let discovered_backend = pixi_build_discovery::DiscoveredBackend::from_package_and_workspace(
        package_manifest.provenance.path.clone(),
        package_manifest,
        &workspace.workspace.value,
        &channel_config,
    )
    .into_diagnostic()?;
    let backend = command_dispatcher
        .instantiate_backend(InstantiateBackendSpec {
            backend_spec: discovered_backend.backend_spec,
            init_params: discovered_backend.init_params,
            channel_config: channel_config.clone(),
            enabled_protocols: EnabledProtocols::default(),
        })
        .await?;

    let incremental = !args.no_incremental;
    let build_dir = args.build_dir.unwrap_or_else(|| workspace.pixi_dir());

    // Determine if we want to re-use existing build data
    let (_tmp, work_dir) = if incremental {
        // Specify the build directory
        let key = WorkDirKey {
            source: Box::new(SourceCheckout::new(
                workspace.root(),
                PinnedSourceSpec::Path(PinnedPathSpec {
                    path: Utf8TypedPath::derive(&workspace.root().to_string_lossy()).to_path_buf(),
                }),
            ))
            .into(),
            host_platform: args.target_platform,
            build_backend: backend.identifier().to_string(),
        }
        .key();

        (None, build_dir.join(key))
    } else {
        // Construct a temporary directory to build the package in. This path is also
        // automatically removed after the build finishes.
        let tmp = tempfile::Builder::new()
            .prefix("pixi-build-")
            .tempdir_in(build_dir)
            .into_diagnostic()
            .context("failed to create temporary working directory in the .pixi directory")?;
        let work_dir = tmp.path().to_path_buf();
        (Some(tmp), work_dir)
    };

    // Build platform virtual packages
    let build_platform_virtual_packages: Vec<GenericVirtualPackage> = workspace
        .default_environment()
        .virtual_packages(Platform::current())
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();

    // Host platform virtual packages
    let host_platform_virtual_packages: Vec<GenericVirtualPackage> = workspace
        .default_environment()
        .virtual_packages(args.target_platform)
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();

    let multi_progress = global_multi_progress();
    let build_context = BuildContext::from_workspace(&workspace, command_dispatcher.clone())?;

    // Build the individual packages.
    let result = backend
        .conda_build(
            CondaBuildParams {
                build_platform_virtual_packages: Some(build_platform_virtual_packages),
                host_platform: Some(PlatformAndVirtualPackages {
                    platform: args.target_platform,
                    virtual_packages: Some(host_platform_virtual_packages),
                }),
                channel_base_urls: Some(
                    workspace
                        .default_environment()
                        .channel_urls(&channel_config)
                        .into_diagnostic()?
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                ),
                channel_configuration: ChannelConfiguration {
                    base_url: channel_config.channel_alias,
                },
                outputs: None,
                editable: false,
                work_directory: work_dir,
                variant_configuration: Some(build_context.resolve_variant(args.target_platform)),
            },
            move |line| {
                let _err = multi_progress.println(line);
            },
        )
        .await
        .wrap_err("during the building of the project the following error occurred")?;

    // Drop the command dispatcher to ensure all resources are cleaned up.
    command_dispatcher.clear_reporter().await;
    drop(build_context);
    drop(command_dispatcher);

    // Move the built packages to the output directory.
    let output_dir = args.output_dir;
    for package in result.packages {
        fs_err::create_dir_all(&output_dir)
            .into_diagnostic()
            .with_context(|| {
                format!(
                    "failed to create output directory '{0}'",
                    output_dir.display()
                )
            })?;

        let file_name = package.output_file.file_name().ok_or_else(|| {
            miette::miette!(
                "output file '{0}' does not have a file name",
                package.output_file.display()
            )
        })?;
        let dest = output_dir.join(file_name);
        if let Err(err) = move_file(&package.output_file, &dest) {
            match err {
                MoveError::CopyFailed(err) => {
                    return Err(err).into_diagnostic().with_context(|| {
                        format!(
                            "failed to copy {} to {}",
                            package.output_file.display(),
                            dest.display()
                        )
                    });
                }
                MoveError::FailedToRemove(e) => {
                    tracing::warn!(
                        "failed to remove {} after copying it to the output directory: {}",
                        package.output_file.display(),
                        e
                    );
                }
                MoveError::MoveFailed(e) => {
                    return Err(e).into_diagnostic().with_context(|| {
                        format!(
                            "failed to move {} to {}",
                            package.output_file.display(),
                            dest.display()
                        )
                    });
                }
            }
        }

        println!(
            "{}Successfully built '{}'",
            console::style(console::Emoji("✔ ", "")).green(),
            dest.display()
        );
    }

    Ok(())
}
