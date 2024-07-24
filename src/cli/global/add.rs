use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::cli::has_specs::HasSpecs;
use crate::config::{Config, ConfigCli};
use crate::progress::global_multi_progress;
use crate::{config, prefix::Prefix, progress::await_in_progress};
use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler::install::{DefaultProgressFormatter, IndicatifReporter, Installer};
use rattler::package_cache::PackageCache;
use rattler_conda_types::{PackageName, Platform, PrefixRecord, RepoDataRecord};
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::Shell,
    shell::ShellEnum,
};
use reqwest_middleware::ClientWithMiddleware;

use super::common::{
    channel_name_from_prefix, find_designated_package, get_client_and_sparse_repodata,
    load_package_records, BinDir, BinEnvDir,
};

/// Adds packages to an environment
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the packages that are to be added.
    #[arg(num_args = 1..)]
    packages: Vec<String>,

    /// Specifies the environment
    #[arg(short, long)]
    environment: String,

    /// Whether to expose binaries
    #[arg(long, default_value_t = true)]
    expose_binares: bool,

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
    // Figure out what channels we are using
    let config = Config::with_cli_config(&args.config);

    let package_names: Result<Vec<PackageName>, _> = args
        .packages()
        .iter()
        .map(|s| PackageName::from_str(s))
        .collect();
    let package_names = package_names.into_diagnostic()?;

    todo!();
    Ok(())
}
