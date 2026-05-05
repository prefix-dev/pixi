use std::path::PathBuf;

use clap::Parser;
use pixi_build_frontend::BackendOverride;
use pixi_config::ConfigCli;
use rattler_conda_types::Platform;

use crate::cli_config::LockAndInstallConfig;

/// Build a conda package from a Pixi package.
#[derive(Parser, Debug)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub config_cli: ConfigCli,

    /// Backend override for testing purposes. This field is ignored by clap
    /// and should only be set programmatically in tests.
    #[clap(skip)]
    pub backend_override: Option<BackendOverride>,

    #[clap(flatten)]
    pub lock_and_install_config: LockAndInstallConfig,

    /// The target platform to build for (defaults to the current platform)
    #[clap(long, short, default_value_t = Platform::current())]
    pub target_platform: Platform,

    /// The build platform to use for building (defaults to the current
    /// platform)
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

    /// The path to a directory containing a package manifest, or to a specific
    /// manifest file.
    ///
    /// Supported manifest files: `package.xml`, `recipe.yaml`, `pixi.toml`,
    /// `pyproject.toml`, or `mojoproject.toml`.
    ///
    /// When a directory is provided, the command will search for supported
    /// manifest files within it.
    #[arg(long)]
    pub path: Option<PathBuf>,
}

pub async fn execute(_: Args) -> miette::Result<()> {
    Err(
        miette::miette!("You can call `pixi publish` for most use cases")
            .wrap_err("`pixi build` has been removed, and will be re-added in future releases"),
    )
}
