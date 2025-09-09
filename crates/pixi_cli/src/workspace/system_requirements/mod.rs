pub mod add;
pub mod list;

use clap::{Parser, ValueEnum};
use pixi_core::WorkspaceLocator;

use crate::cli_config::WorkspaceConfig;

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

/// Commands to manage workspace system requirements.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

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
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    match args.command {
        Command::Add(args) => add::execute(workspace, args).await,
        Command::List(args) => list::execute(&workspace, args),
    }
}
