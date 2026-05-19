use std::path::{Path, PathBuf};

use clap::Parser;
use pixi_build_frontend::BackendOverride;
use pixi_config::ConfigCli;
use rattler_conda_types::Platform;

use crate::cli_config::LockAndInstallConfig;
use crate::publish;

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

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut cmd_parts = vec!["pixi publish".to_string()];

    if args.target_platform != Platform::current() {
        cmd_parts.push(format!("--target-platform {}", args.target_platform));
    }
    if args.build_platform != Platform::current() {
        cmd_parts.push(format!("--build-platform {}", args.build_platform));
    }
    if let Some(ref build_dir) = args.build_dir {
        cmd_parts.push(format!("--build-dir {}", build_dir.display()));
    }
    if args.clean {
        cmd_parts.push("--clean".to_string());
    }
    if let Some(ref path) = args.path {
        cmd_parts.push(format!("--path {}", path.display()));
    }
    if args.output_dir != Path::new(".") {
        cmd_parts.push(format!("--target-dir {}", args.output_dir.display()));
    }

    let equivalent_cmd = cmd_parts.join(" ");

    eprintln!(
        "\n{} `pixi build` is deprecated and will be removed in a future release.",
        console::style("WARNING:").yellow().bold()
    );
    eprintln!(
        "Use `pixi publish` instead. Equivalent command:\n\n  {}\n",
        console::style(&equivalent_cmd).cyan()
    );

    publish::execute(publish::Args {
        config_cli: args.config_cli,
        backend_override: args.backend_override,
        target_platform: args.target_platform,
        build_platform: args.build_platform,
        build_string_prefix: None,
        build_number: None,
        build_dir: args.build_dir,
        clean: args.clean,
        path: args.path,
        target_channel: None,
        target_dir: Some(args.output_dir),
        force: false,
        skip_existing: true,
        generate_attestation: false,
        variant: Vec::new(),
        variant_config: Vec::new(),
    })
    .await
}
