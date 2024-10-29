use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::HashSet,
    io::{stdout, Write},
};

use crate::cli::cli_config::ProjectConfig;
use crate::{
    load_lock_file,
    lock_file::{filter_lock_file, UpdateContext},
    Project,
};
use ahash::HashMap;
use clap::{builder::Str, Parser};
use fancy_display::FancyDisplay;
use indexmap::IndexMap;
use itertools::{Either, Itertools};
use miette::{Context, IntoDiagnostic, MietteDiagnostic};
use pep508_rs::PackageName;
use pixi_config::ConfigCli;

use pixi_consts::consts;
use pixi_manifest::FeaturesExt;
use pixi_manifest::{EnvironmentName, FeatureName};
use rattler_conda_types::Platform;
use rattler_lock::{LockFile, Package};
use serde::Serialize;
use serde_json::Value;
use tabwriter::TabWriter;

/// Update dependencies as recorded in the local lock file
#[derive(Parser, Debug, Default)]
pub struct Args {
    #[clap(flatten)]
    pub config: ConfigCli,

    #[clap(flatten)]
    pub project_config: ProjectConfig,

    #[clap(flatten)]
    pub specs: UpgradeSpecsArgs,
}

#[derive(Parser, Debug, Default)]
pub struct UpgradeSpecsArgs {
    /// The packages to upgrade
    pub packages: Option<Vec<String>>,

    /// The feature to update
    #[clap(long = "feature", short = 'f')]
    pub feature: Option<FeatureName>,
}

/// A distilled version of `UpgradeSpecsArgs`.
struct UpgradeSpecs {
    packages: Option<HashSet<String>>,
    feature: Option<FeatureName>,
}

impl From<UpgradeSpecsArgs> for UpgradeSpecs {
    fn from(args: UpgradeSpecsArgs) -> Self {
        Self {
            packages: args.packages.map(|args| args.into_iter().collect()),
            feature: args.feature,
        }
    }
}

impl UpgradeSpecs {
    /// Returns true if the package should be relaxed according to the user
    /// input.
    fn should_relax(&self, feature_name: &str, package: &Package) -> bool {
        todo!()
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = args.config;
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?
        .with_cli_config(config);

    let specs = UpgradeSpecs::from(args.specs);

    // If the user specified a feature name, check to see if it exists.
    if let Some(feature) = &specs.feature {
        if project.manifest.feature(feature).is_none() {
            miette::bail!("could not find a feature named {}", feature.fancy_display())
        }
    }

    // If the user specified a package name, check to see if it is even there.
    if let Some(packages) = &specs.packages {
        for package in packages {
            ensure_package_exists(&package, &specs)?
        }
    }

    todo!()
}

/// Ensures the existence of the specified package
///
/// # Returns
///
/// Returns `miette::Result` with a descriptive error message
/// if the package does not exist.
fn ensure_package_exists(package_name: &str, specs: &UpgradeSpecs) -> miette::Result<()> {
    let environments = lock_file
        .environments()
        .filter_map(|(name, env)| {
            if let Some(envs) = &specs.environments {
                if !envs.contains(name) {
                    return None;
                }
            }
            Some(env)
        })
        .collect_vec();

    let similar_names = environments
        .iter()
        .flat_map(|env| env.packages_by_platform())
        .filter_map(|(p, packages)| {
            if let Some(platforms) = &specs.platforms {
                if !platforms.contains(&p) {
                    return None;
                }
            }
            Some(packages)
        })
        .flatten()
        .map(|p| p.name().to_string())
        .unique()
        .filter_map(|name| {
            let distance = strsim::jaro(package_name, &name);
            if distance > 0.6 {
                Some((name, distance))
            } else {
                None
            }
        })
        .sorted_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(Ordering::Equal))
        .take(5)
        .map(|(name, _)| name)
        .collect_vec();

    if similar_names.first().map(String::as_str) == Some(package_name) {
        return Ok(());
    }

    let message = format!("could not find a package named '{package_name}'");

    Err(MietteDiagnostic {
        message,
        code: None,
        severity: None,
        help: if !similar_names.is_empty() {
            Some(format!(
                "did you mean '{}'?",
                similar_names.iter().format("', '")
            ))
        } else {
            None
        },
        url: None,
        labels: None,
    }
    .into())
}
