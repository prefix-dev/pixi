use std::path::PathBuf;

use clap::Parser;
use miette::{Context, IntoDiagnostic};
use pixi_build_frontend::SetupRequest;
use pixi_build_types::{
    procedures::conda_build::CondaBuildParams, ChannelConfiguration, PlatformAndVirtualPackages,
};
use pixi_config::ConfigCli;
use pixi_manifest::FeaturesExt;
use rattler_conda_types::Platform;

use crate::{
    cli::cli_config::ProjectConfig,
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
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?
        .with_cli_config(args.config_cli);

    // TODO: Implement logic to take the source code from a VCS instead of from a
    // local channel so that that information is also encoded in the manifest.

    // Instantiate a protocol for the source directory.
    let channel_config = project.channel_config();
    let protocol = pixi_build_frontend::BuildFrontend::default()
        .with_channel_config(channel_config.clone())
        .setup_protocol(SetupRequest {
            source_dir: project.root().to_path_buf(),
            build_tool_overrides: Default::default(),
        })
        .await
        .into_diagnostic()
        .wrap_err("unable to setup the build-backend to build the project")?;

    // Build the individual packages.
    let result = protocol
        .conda_build(&CondaBuildParams {
            build_platform_virtual_packages: None,
            host_platform: Some(PlatformAndVirtualPackages {
                platform: args.target_platform,
                virtual_packages: None,
            }),
            channel_base_urls: Some(
                project
                    .default_environment()
                    .channels()
                    .iter()
                    .map(|&c| c.clone().into_base_url(&channel_config))
                    .collect::<Result<Vec<_>, _>>()
                    .into_diagnostic()?,
            ),
            channel_configuration: ChannelConfiguration {
                base_url: channel_config.channel_alias,
            },
            outputs: None,
        })
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
