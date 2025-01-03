pub mod add;
pub mod list;

use crate::cli::cli_config::ProjectConfig;
use crate::Project;
use clap::{Parser, ValueEnum};

/// Enum for valid system requirement names.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SystemRequirementEnum {
    /// The version of the linux kernel (Find with `uname -r`)
    Linux,
    /// The version of the CUDA driver (Find with `nvidia-smi`)
    Cuda,
    /// The version of MacOS (Find with `sw_vers`)
    Macos,
    /// The version of the glibc library (Find with `ldd --version`)
    Glibc,
    /// Non Glibc libc family and version (Find with `ldd --version`)
    OtherLibc,
    // Not in use yet
    // ArchSpec,
}

/// Commands to manage project environments.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: ProjectConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Adds an environment to the manifest file.
    #[clap(visible_alias = "a")]
    Add(add::Args),
    /// List the environments in the manifest file.
    #[clap(visible_alias = "ls")]
    List(list::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?;

    match args.command {
        Command::Add(args) => add::execute(project, args).await,
        Command::List(args) => list::execute(&project, args),
    }
}
