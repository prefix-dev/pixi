use std::{path::PathBuf, sync::Arc, time::Duration};

use clap::Parser;
use indicatif::ProgressBar;
use miette::{Context, IntoDiagnostic};
use pixi_build_frontend::{BackendOverride, CondaBuildReporter, SetupRequest};
use pixi_build_types::{
    procedures::conda_build::CondaBuildParams, ChannelConfiguration, PlatformAndVirtualPackages,
};
use pixi_config::ConfigCli;
use pixi_manifest::FeaturesExt;
use rattler_conda_types::{GenericVirtualPackage, Platform};

use crate::{
    build::BuildContext,
    cli::cli_config::WorkspaceConfig,
    repodata::Repodata,
    utils::{move_file, MoveError},
    WorkspaceLocator,
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

    /// The output directory to place the build artifacts
    #[clap(long, short, default_value = ".")]
    pub output_dir: PathBuf,
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
            build_tool_override: BackendOverride::from_env(),
            build_id: 0,
        })
        .await
        .into_diagnostic()
        .wrap_err("unable to setup the build-backend to build the workspace")?;

    // Construct a temporary directory to build the package in. This path is also
    // automatically removed after the build finishes.
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

    let work_dir = tempfile::Builder::new()
        .prefix("pixi-build-")
        .tempdir_in(workspace.pixi_dir())
        .into_diagnostic()
        .context("failed to create temporary working directory in the .pixi directory")?;

    let progress = Arc::new(ProgressReporter::new(workspace.name()));
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

    let build_context = BuildContext::from_workspace(&workspace)?;

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
                work_directory: work_dir.path().to_path_buf(),
                variant_configuration: Some(build_context.resolve_variant(args.target_platform)),
            },
            progress.clone(),
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
                    })
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
