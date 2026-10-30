use clap::Parser;
use pixi_config::ConfigCli;
use rattler_conda_types::Subdir;

use crate::cli_config::ChannelsConfig;

/// Upgrade all globally installed packages
/// This command has been removed, please use `pixi global update` instead
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    channels: ChannelsConfig,

    #[clap(flatten)]
    config: ConfigCli,

    /// The platform to install the package for.
    #[clap(long, default_value_t = Subdir::current().unwrap_or(Subdir::NoArch))]
    platform: Subdir,
}

pub async fn execute(_args: Args) -> miette::Result<()> {
    Err(
        miette::miette!("You can call `pixi global update` for most use cases")
            .wrap_err("`pixi global upgrade-all` has been removed"),
    )
}
