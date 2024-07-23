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

/// Syncs the global environments with the manifest
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    #[clap(flatten)]
    config: ConfigCli,
}

/// Install a global command
pub async fn execute(args: Args) -> miette::Result<()> {
    // Figure out what channels we are using
    let config = Config::with_cli_config(&args.config);

    let manifest = super::manifest::read_global_manifest();
    manifest.setup_envs().await.unwrap();
    Ok(())
}
