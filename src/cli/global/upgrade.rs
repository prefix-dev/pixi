use clap::Parser;
use rattler_conda_types::Platform;

use crate::cli::{cli_config::ChannelsConfig, has_specs::HasSpecs};

/// Upgrade specific package which is installed globally.
/// This command has been removed, please use `pixi global update` instead
#[derive(Parser, Debug)]
// TODO: Uncomment as soon we implement this
//#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the packages to upgrade.
    // TODO: Uncomment as soon we implement this
    //#[arg(required = true)]
    pub specs: Vec<String>,

    #[clap(flatten)]
    channels: ChannelsConfig,

    /// The platform to install the package for.
    #[clap(long, default_value_t = Platform::current())]
    platform: Platform,
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.specs.iter().map(AsRef::as_ref).collect()
    }
}

pub async fn execute(_args: Args) -> miette::Result<()> {
    Err(
        miette::miette!("You can call `pixi global update` for most use cases").wrap_err(
            "`pixi global upgrade` has been removed, and will be re-added in future releases",
        ),
    )
}
