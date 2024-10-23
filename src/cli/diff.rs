use std::path::{Path, PathBuf};

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_config::{self, ConfigCli};
use rattler_lock::LockFile;

use crate::{
    cli::update::{LockFileDiff, LockFileJsonDiff},
    Project,
};

use super::cli_config::ProjectConfig;

#[derive(Parser, Debug, Default)]
pub struct Args {
    #[clap(flatten)]
    pub config: ConfigCli,

    #[clap(flatten)]
    pub project_config: ProjectConfig,

    #[arg(long)]
    pub old_lockfile: PathBuf,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let lock = LockFile::from_path(Path::new("pixi.lock")).into_diagnostic()?;

    let input: Box<dyn std::io::Read + 'static> = if args.old_lockfile.as_os_str() == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(std::fs::File::open(&args.old_lockfile).into_diagnostic()?)
    };

    let old = LockFile::from_reader(input).into_diagnostic()?;

    let diff = LockFileDiff::from_lock_files(&old, &lock);
    let config = args.config;
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?
        .with_cli_config(config);
    let json_diff = LockFileJsonDiff::new(&project, diff);
    let json = serde_json::to_string_pretty(&json_diff).expect("failed to convert to json");
    println!("{}", json);

    Ok(())
}
