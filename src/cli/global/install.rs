use clap::Parser;
use rattler_conda_types::{package, Platform};

use crate::{
    cli::{cli_config::ChannelsConfig, has_specs::HasSpecs},
    global::{self, EnvironmentName},
};
use pixi_config::{self, Config, ConfigCli};

/// Installs the defined package in a global accessible location.
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the packages that are to be installed.
    #[arg(num_args = 1..)]
    packages: Vec<String>,

    #[clap(flatten)]
    channels: ChannelsConfig,

    #[clap(short, long)]
    platform: Option<Platform>,

    /// Ensures that all packages will be installed in the same environment
    #[clap(short, long)]
    environment: Option<EnvironmentName>,

    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,

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
    let config = Config::with_cli_config(&args.config);

    global::sync(&config, args.assume_yes).await?;

    Ok(())
}
