use std::{path::PathBuf, sync::Arc, time::Duration};

use clap::Parser;
use indicatif::ProgressBar;
use miette::{Context, IntoDiagnostic};
use pixi_build_frontend::{BackendOverride, CondaBuildReporter, SetupRequest};
use pixi_build_types::{
    ChannelConfiguration, PlatformAndVirtualPackages, procedures::conda_build::CondaBuildParams,
};
use pixi_command_dispatcher::SourceCheckout;
use pixi_config::ConfigCli;
use pixi_manifest::FeaturesExt;
use pixi_progress::global_multi_progress;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec};
use rattler_conda_types::{GenericVirtualPackage, Platform};
use typed_path::Utf8TypedPath;

use crate::{
    WorkspaceLocator,
    build::{BuildContext, WorkDirKey},
    cli::cli_config::WorkspaceConfig,
    repodata::Repodata,
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

struct ProgressReporter {
    progress_bar: indicatif::ProgressBar,
}

impl ProgressReporter {
    fn new(source: &str) -> Self {
        let style = indicatif::ProgressStyle::default_bar()
            .template("{spinner:.dim} {elapsed} {prefix} {wide_msg:.dim}")
            .expect("should be able to create a progress bar style");
        let pb = ProgressBar::new(0);
        pb.set_style(style);
        let progress = pixi_progress::global_multi_progress().add(pb);
        progress.set_prefix(format!("building package: {}", source));
        progress.enable_steady_tick(Duration::from_millis(100));

        Self {
            progress_bar: progress,
        }
    }
}

impl CondaBuildReporter for ProgressReporter {
    /// Starts a progress bar that should currently be
    ///  [spinner] message
    fn on_build_start(&self, _build_id: usize) -> usize {
        // Create a new progress bar.
        // Building the package
        0
    }

    fn on_build_end(&self, _operation: usize) {
        // Finish the progress bar.
        self.progress_bar.finish_with_message("build completed");
    }

    fn on_build_output(&self, _operation: usize, line: String) {
        self.progress_bar.suspend(|| eprintln!("{}", line))
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.project_config.workspace_locator_start())
        .with_closest_package(true)
        .locate()?
        .with_cli_config(args.config_cli);

    // TODO: Implement logic to take the source code from a VCS instead of from a
    // local channel so that that information is also encoded in the manifest.

    // Instantiate a protocol for the source directory.
    let channel_config = workspace.channel_config();

    let tool_context = pixi_build_frontend::ToolContext::builder()
        .with_gateway(workspace.repodata_gateway()?.clone())
        .with_client(workspace.authenticated_client()?.clone())
        .build();

    let protocol = pixi_build_frontend::BuildFrontend::default()
        .with_channel_config(channel_config.clone())
        .with_tool_context(Arc::new(tool_context))
        .setup_protocol(SetupRequest {
            source_dir: workspace
                .package
                .as_ref()
                .map(|pkg| &pkg.provenance.path)
                .unwrap_or(&workspace.workspace.provenance.path)
                .parent()
                .expect("a manifest must have parent directory")
                .to_path_buf(),
            build_tool_override: BackendOverride::from_env()?,
            build_id: 0,
        })
        .await
        .into_diagnostic()
        .wrap_err("unable to setup the build-backend to build the workspace")?;

    let pixi_dir = &workspace.pixi_dir();
    tokio::fs::create_dir_all(pixi_dir)
        .await
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to create the .pixi directory at '{}'",
                pixi_dir.display()
            )
        })?;

    // Create the build directory if it does not exist
    if let Some(build_dir) = args.build_dir.as_ref() {
        tokio::fs::create_dir_all(build_dir)
            .await
            .into_diagnostic()
            .with_context(|| {
                format!(
                    "failed to create the build directory at '{}'",
                    build_dir.display()
                )
            })?;
    }

    let incremental = !args.no_incremental;
    let build_dir = args.build_dir.unwrap_or_else(|| workspace.pixi_dir());
    // Determine if we want to re-use existing build data
    let (_tmp, work_dir) = if incremental {
        // Specify the build directory
        let key = WorkDirKey::new(
            SourceCheckout::new(
                workspace.root(),
                PinnedSourceSpec::Path(PinnedPathSpec {
                    path: Utf8TypedPath::derive(&workspace.root().to_string_lossy()).to_path_buf(),
                }),
            ),
            args.target_platform,
            protocol.backend_identifier().to_string(),
        )
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

    let progress = ProgressReporter::new(workspace.display_name());

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
    let anchor_pb = multi_progress.add(ProgressBar::hidden());
    let command_dispatcher = workspace
        .command_dispatcher_builder()?
        .with_reporter(TopLevelProgress::new(global_multi_progress(), anchor_pb))
        .finish();

    let build_context = BuildContext::from_workspace(&workspace, command_dispatcher)?;

    // Build the individual packages.
    let result = protocol
        .conda_build(
            &CondaBuildParams {
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
            &progress,
        )
        .await
        .wrap_err("during the building of the project the following error occurred")?;

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
