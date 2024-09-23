use clap::Parser;
use rattler_conda_types::Platform;

use crate::{cli::cli_config::ChannelsConfig, cli::has_specs::HasSpecs};
use pixi_config::{self, ConfigCli};

/// Installs the defined package in a global accessible location.
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the packages that are to be installed.
    #[arg(num_args = 1..)]
    packages: Vec<String>,

    #[clap(flatten)]
    channels: ChannelsConfig,

    #[clap(short, long, default_value_t = Platform::current())]
    platform: Platform,

    #[clap(flatten)]
    config: ConfigCli,

    /// Do not insert `CONDA_PREFIX`, `PATH` modifications into the installed executable script.
    #[clap(long)]
    no_activation: bool,
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.packages.iter().map(AsRef::as_ref).collect()
    }
}

/// Install a global command
pub async fn execute(_args: Args) -> miette::Result<()> {
    todo!()
}
