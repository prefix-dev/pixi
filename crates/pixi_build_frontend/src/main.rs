//! Contains a simple CLI for testing the build-frontend on arbitrary locations
use std::path::PathBuf;

use clap::Parser;
use pixi_build_frontend::{BackendOverrides, BuildFrontend, BuildRequest, CondaMetadataRequest};
use rattler_conda_types::{ChannelConfig, MatchSpec};
use tracing_subscriber::{
    fmt, fmt::writer::MakeWriterExt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

/// CLI options for the build frontend. These are used to override values from
/// a manifest to specify the build tool to use.
#[derive(Parser, Debug)]
pub struct BuilderOptions {
    #[arg(short = 's', long, conflicts_with("build_tool_path"))]
    /// Override the build tool with a specific conda package
    pub build_tool_spec: Option<MatchSpec>,

    #[arg(short = 'p', long)]
    /// Override the build tool with a specific binary
    pub build_tool_path: Option<PathBuf>,
}

impl From<BuilderOptions> for BackendOverrides {
    fn from(value: BuilderOptions) -> Self {
        Self {
            spec: value.build_tool_spec,
            path: value.build_tool_path,
        }
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None, arg_required_else_help = true,)]
/// Runs the build tool found in the toml file for a specified source
struct Args {
    #[clap(flatten)]
    builder_opts: BuilderOptions,

    /// Tries to find manifest in the specified directory
    #[arg(default_value = ".")]
    work_dir: PathBuf,
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());

    // Construct a frontend, this could be a global instance in a real application.
    // It mainly stores caches.
    let frontend = BuildFrontend::default().with_channel_config(channel_config);

    // Build a specific package.
    let protocol = frontend
        .protocol(BuildRequest {
            source_dir: args.work_dir,
            build_tool_overrides: args.builder_opts.into(),
        })
        .await?;

    // Request package metadata
    let metadata = protocol
        .get_conda_metadata(&CondaMetadataRequest {
            channel_base_urls: vec!["https://conda.anaconda.org/conda-forge".parse().unwrap()],
        })
        .await?;

    dbg!(metadata);

    Ok(())
}
