use std::cmp::Ordering;
use std::sync::Arc;

use crate::cli::cli_config::ProjectConfig;
use crate::Project;
use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::IntoDiagnostic;
use miette::MietteDiagnostic;

use pep508_rs::MarkerTree;
use pep508_rs::Requirement;
use pixi_manifest::FeatureName;
use pixi_manifest::PyPiRequirement;
use pixi_manifest::SpecType;
use pixi_spec::PixiSpec;
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

    // TODO: Also support build and host
    let spec_type = SpecType::Run;
    let match_spec_iter = feature
        .dependencies(Some(spec_type), None)
        .into_iter()
        .flat_map(|deps| deps.into_owned());

    let pypi_deps_iter = feature
        .pypi_dependencies(None)
        .into_iter()
        .flat_map(|deps| deps.into_owned());

    // If the user specified a package name, check to see if it is even there.
    if let Some(package_names) = &args.specs.packages {
        let available_packages = match_spec_iter
            .clone()
            .map(|(name, _)| name.as_normalized().to_string())
            .chain(
                pypi_deps_iter
                    .clone()
                    .map(|(name, _)| name.as_normalized().to_string()),
            )
            .collect_vec();

        for package in package_names {
            ensure_package_exists(package, &available_packages)?
        }
    }

    let match_specs = match_spec_iter
        // If specific packages have been requested, only upgrade those
        .filter(|(name, _)| match &args.specs.packages {
            None => true,
            Some(packages) if packages.contains(&name.as_normalized().to_string()) => true,
            _ => false,
        })
        // Only upgrade version specs
        .filter_map(|(name, req)| match req {
            PixiSpec::DetailedVersion(version_spec) => {
                let channel = version_spec
                    .channel
                    .and_then(|c| c.into_channel(&project.channel_config()).ok())
                    .map(Arc::new);
                Some((
                    name.clone(),
                    (
                        MatchSpec {
                            name: Some(name),
                            channel,
                            ..Default::default()
                        },
                        spec_type,
                    ),
                ))
            }
            PixiSpec::Version(_) => Some((name.clone(), (MatchSpec::from(name), spec_type))),
            _ => None,
        })
        .collect();

    let pypi_deps = pypi_deps_iter
        // If specific packages have been requested, only upgrade those
        .filter(|(name, _)| match &args.specs.packages {
            None => true,
            Some(packages) if packages.contains(&name.as_normalized().to_string()) => true,
            _ => false,
        })
        // Only upgrade version specs
        .filter_map(|(name, req)| match req {
            PyPiRequirement::Version { extras, .. } => Some((
                name.clone(),
                Requirement {
                    name: name.as_normalized().clone(),
                    extras,
                    marker: MarkerTree::default(),
                    origin: None,
                    version_or_url: None,
                },
            )),
            PyPiRequirement::RawVersion(_) => Some((
                name.clone(),
                Requirement {
                    name: name.as_normalized().clone(),
                    extras: Vec::default(),
                    marker: MarkerTree::default(),
                    origin: None,
                    version_or_url: None,
                },
            )),
            _ => None,
        })
        .collect();

    let update_deps = project
        .update_dependencies(
            match_specs,
            pypi_deps,
            &args.prefix_update_config,
            &args.specs.feature,
            &[],
            false,
        )
        .await?;

    if let Some(update_deps) = update_deps {
        update_deps.lock_file_diff.print().into_diagnostic()?;
    }

    Project::warn_on_discovered_from_env(args.project_config.manifest_path.as_deref());
    Ok(())
}

/// Ensures the existence of the specified package
///
/// # Returns
///
/// Returns `miette::Result` with a descriptive error message
/// if the package does not exist.
fn ensure_package_exists(package_name: &str, available_packages: &[String]) -> miette::Result<()> {
    let similar_names = available_packages
        .iter()
        .unique()
        .filter_map(|name| {
            let distance = strsim::jaro(package_name, name);
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

    if similar_names.first().map(|s| s.as_str()) == Some(package_name) {
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
