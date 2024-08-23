use std::collections::HashSet;

use clap::Parser;
use clap_verbosity_flag::{Level, Verbosity};
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::PackageName;

use crate::cli::has_specs::HasSpecs;
use crate::global::install::ScriptExecMapping;
use crate::prefix::Prefix;

use crate::global::{find_designated_package, BinDir, BinEnvDir};

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

pub async fn execute(args: Args) -> miette::Result<()> {
    todo!()
}
