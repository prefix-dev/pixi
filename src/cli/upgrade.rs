use std::{cmp::Ordering, collections::HashSet};

use crate::cli::cli_config::ProjectConfig;
use crate::load_lock_file;
use crate::Project;
use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::MietteDiagnostic;
use pixi_config::ConfigCli;

use pixi_manifest::Feature;
use pixi_manifest::FeatureName;
use rattler_lock::Package;

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
    #[clap(long = "feature", short = 'f', default_value_t)]
    pub feature: FeatureName,
}

/// A distilled version of `UpgradeSpecsArgs`.
struct UpgradeSpecs {
    packages: Option<HashSet<String>>,
    feature: FeatureName,
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

    // Ensure that the given feature exists
    let Some(feature) = project.manifest.feature(&specs.feature) else {
        miette::bail!(
            "could not find a feature named {}",
            specs.feature.fancy_display()
        )
    };

    // If the user specified a package name, check to see if it is even there.
    if let Some(packages) = &specs.packages {
        for package in packages {
            ensure_package_exists(feature, package)?
        }
    }

    // Load the current lock-file, if any. If none is found, a dummy lock-file is
    // returned.
    let loaded_lock_file = load_lock_file(&project).await?;

    todo!()
}

/// Ensures the existence of the specified package
///
/// # Returns
///
/// Returns `miette::Result` with a descriptive error message
/// if the package does not exist.
fn ensure_package_exists(feature: &Feature, package_name: &str) -> miette::Result<()> {
    let similar_names = feature
        .dependencies(None, None)
        .into_iter()
        .flat_map(|deps| deps.into_owned())
        .map(|(name, _)| name.as_normalized().to_string())
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
