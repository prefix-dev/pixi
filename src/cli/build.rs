use std::{path::PathBuf, sync::Arc, time::Duration};

use clap::{ArgAction, Parser};
use indicatif::ProgressBar;
use miette::{Context, IntoDiagnostic};
use pixi_build_frontend::{CondaBuildReporter, EnabledProtocols, SetupRequest};
use pixi_build_types::{
    procedures::conda_build::CondaBuildParams, ChannelConfiguration, PlatformAndVirtualPackages,
};
use pixi_config::ConfigCli;
use pixi_manifest::FeaturesExt;
use rattler_conda_types::{GenericVirtualPackage, Platform};

use crate::{
    cli::cli_config::ProjectConfig,
    repodata::Repodata,
    utils::{move_file, MoveError},
    Project,
};

#[derive(Parser, Debug)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: ProjectConfig,

    #[clap(flatten)]
    pub config_cli: ConfigCli,

    /// The target platform to build for (defaults to the current platform)
    #[clap(long, short, default_value_t = Platform::current())]
    pub target_platform: Platform,

    /// The output directory to place the build artifacts
    #[clap(long, short, default_value = ".")]
    pub output_dir: PathBuf,

    /// Use system backend installed tool
    #[arg(long, action = ArgAction::SetTrue)]
    pub with_system: bool,

    /// If a recipe.yaml is present in the source directory, ignore it
    /// and build the package using manifest only
    #[arg(long, action = ArgAction::SetTrue)]
    pub ignore_recipe: bool,
}

struct ProgressReporter {
    progress_bar: indicatif::ProgressBar,
}

impl ProgressReporter {
    fn new(source: &str) -> Self {
        let style = indicatif::ProgressStyle::default_bar()
            .template("{spinner:.dim} {elapsed} {prefix} {wide_msg:.dim}")
            .unwrap();
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
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?
        .with_cli_config(args.config_cli);

    // TODO: Implement logic to take the source code from a VCS instead of from a
    // local channel so that that information is also encoded in the manifest.

    // Instantiate a protocol for the source directory.
    let channel_config = project.channel_config();

    let tool_context = pixi_build_frontend::ToolContext::builder()
        .with_gateway(project.repodata_gateway().clone())
        .with_client(project.authenticated_client().clone())
        .build();

    let enabled_protocols = EnabledProtocols {
        enable_rattler_build: !args.ignore_recipe,
        ..Default::default()
    };

    let protocol = pixi_build_frontend::BuildFrontend::default()
        .with_channel_config(channel_config.clone())
        .with_tool_context(Arc::new(tool_context))
        .with_enabled_protocols(enabled_protocols)
        .setup_protocol(SetupRequest {
            source_dir: project.root().to_path_buf(),
            build_tool_override: None,
            build_id: 0,
        })
        .await
        .into_diagnostic()
        .wrap_err("unable to setup the build-backend to build the project")?;

    // Construct a temporary directory to build the package in. This path is also
    // automatically removed after the build finishes.
    let pixi_dir = &project.pixi_dir();
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
        .tempdir_in(project.pixi_dir())
        .into_diagnostic()
        .context("failed to create temporary working directory in the .pixi directory")?;

    let progress = Arc::new(ProgressReporter::new(project.name()));
    // Build platform virtual packages
    let build_platform_virtual_packages: Vec<GenericVirtualPackage> = project
        .default_environment()
        .virtual_packages(Platform::current())
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();

    // Host platform virtual packages
    let host_platform_virtual_packages: Vec<GenericVirtualPackage> = project
        .default_environment()
        .virtual_packages(args.target_platform)
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();

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
                    project
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
                work_directory: work_dir.path().to_path_buf(),
            },
            progress.clone(),
        )
        .await
        .wrap_err("during the building of the project the following error occurred")?;

    // Move the built packages to the output directory.
    let output_dir = args.output_dir;
    for package in result.packages {
        std::fs::create_dir_all(&output_dir)
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
