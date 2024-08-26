use clap::Parser;
use clap_verbosity_flag::Verbosity;

use crate::cli::has_specs::HasSpecs;

/// Removes a package previously installed into a globally accessible location via `pixi global install`.
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the packages that are to be removed.
    #[arg(num_args = 1..)]
    packages: Vec<String>,

    #[command(flatten)]
    verbose: Verbosity,
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.packages.iter().map(AsRef::as_ref).collect()
    }
}

pub async fn execute(_args: Args) -> miette::Result<()> {
    todo!()
}
