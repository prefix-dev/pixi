use std::cmp::Ordering;
use std::str::FromStr;

use crate::cli::cli_config::ProjectConfig;
use crate::Project;
use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::MietteDiagnostic;

use pixi_manifest::Feature;
use pixi_manifest::FeatureName;
use pixi_manifest::SpecType;
use rattler_conda_types::MatchSpec;

use super::cli_config::PrefixUpdateConfig;

/// Update dependencies as recorded in the local lock file
#[derive(Parser, Debug, Default)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: ProjectConfig,

    #[clap(flatten)]
    pub prefix_update_config: PrefixUpdateConfig,

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

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?
        .with_cli_config(args.prefix_update_config.config.clone());

    // Ensure that the given feature exists
    let Some(feature) = project.manifest.feature(&args.specs.feature) else {
        miette::bail!(
            "could not find a feature named {}",
            args.specs.feature.fancy_display()
        )
    };

    // If the user specified a package name, check to see if it is even there.
    if let Some(packages) = &args.specs.packages {
        for package in packages {
            ensure_package_exists(feature, package)?
        }
    }

    // TODO: Also support build and host
    let spec_type = SpecType::Run;
    let match_specs = feature
        .dependencies(Some(spec_type), None)
        .into_iter()
        .flat_map(|deps| deps.into_owned())
        .filter(|(name, _)| match &args.specs.packages {
            None => true,
            Some(packages) if packages.contains(&name.as_normalized().to_string()) => true,
            _ => false,
        })
        .filter_map(|(name, spec)| {
            match spec.try_into_nameless_match_spec(&project.channel_config()) {
                Ok(Some(mut nameless_spec)) => {
                    // In order to upgrade, we remove the version requirement
                    nameless_spec.version = None;
                    Some((
                        name.clone(),
                        (
                            MatchSpec::from_nameless(nameless_spec, Some(name)),
                            spec_type,
                        ),
                    ))
                }
                _ => None,
            }
        })
        .collect();

    let pypi_deps = feature
        .pypi_dependencies(None)
        .into_iter()
        .flat_map(|deps| deps.into_owned())
        .filter(|(name, _)| match &args.specs.packages {
            None => true,
            Some(packages) if packages.contains(&name.as_normalized().to_string()) => true,
            _ => false,
        })
        .filter_map(
            |(name, req)| match pep508_rs::Requirement::from_str(&req.to_string()) {
                Ok(pep_req) => Some((name, pep_req)),
                _ => None,
            },
        )
        .collect();

    project
        .update_dependencies(
            match_specs,
            pypi_deps,
            &args.prefix_update_config,
            &args.specs.feature,
            &[],
            false,
        )
        .await?;

    Project::warn_on_discovered_from_env(args.project_config.manifest_path.as_deref());
    Ok(())
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
