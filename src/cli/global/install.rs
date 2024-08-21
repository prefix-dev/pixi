use clap::Parser;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pixi_utils::reqwest::build_reqwest_clients;
use rattler_conda_types::{GenericVirtualPackage, Platform};

use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::VirtualPackage;

use crate::global::{
    channel_name_from_prefix, install::prompt_user_to_continue, print_executables_available,
};
use crate::{cli::cli_config::ChannelsConfig, cli::has_specs::HasSpecs};
use pixi_config::{self, Config, ConfigCli};
use pixi_progress::wrap_in_progress;

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
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.packages.iter().map(AsRef::as_ref).collect()
    }
}

/// Install a global command
pub async fn execute(args: Args) -> miette::Result<()> {
    todo!()
}
