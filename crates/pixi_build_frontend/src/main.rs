//! Contains a simple CLI for testing the build-frontend on arbitrary locations
use clap::Parser;
use miette::IntoDiagnostic;
use pixi_build_frontend::options::{BuildToolSpec, PixiBuildFrontendOptions};
use pixi_build_frontend::{build, BuildToolInfo};
use rattler_conda_types::MatchSpec;
use std::path::PathBuf;

#[derive(Parser, Debug)]
/// CLI options for the build frontend
pub struct OptionsCLI {
    #[arg(short = 's', long, conflicts_with("build_tool_path"))]
    /// Override the build tool with a specific conda package
    pub build_tool_spec: Option<MatchSpec>,
    #[arg(short = 'p', long)]
    /// Override the build tool with a specific binary
    pub build_tool_path: Option<PathBuf>,
}

impl From<OptionsCLI> for PixiBuildFrontendOptions {
    fn from(cli: OptionsCLI) -> Self {
        PixiBuildFrontendOptions {
            override_build_tool: cli
                .build_tool_path
                .map(|s| BuildToolSpec::DirectBinary(s))
                .or(cli.build_tool_spec.map(|s| BuildToolSpec::CondaPackage(s))),
        }
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None, arg_required_else_help = true,)]
/// Runs the build tool found in the toml file for a specified source
struct Args {
    #[clap(flatten)]
    global_opts: OptionsCLI,

    /// Tries to find manifest in the specified directory
    #[arg(default_value = ".")]
    work_dir: PathBuf,
}

fn main() -> miette::Result<()> {
    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
    tracing::info!("starting build frontend");
    let options = Args::parse();
    let manifest_path = options.work_dir.join("pixi.toml");
    let build_tool_info = BuildToolInfo::from_pixi(&manifest_path, &options.global_opts.into())?;
    tracing::info!("Will build with: {:?}", build_tool_info.build_tool);
    let result = build(&build_tool_info, &options.work_dir).into_diagnostic()?;

    if result.success {
        tracing::info!("Build successful");
    } else {
        tracing::error!("Build failed");
    }

    Ok(())
}
