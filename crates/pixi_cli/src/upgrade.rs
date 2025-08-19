use std::cmp::Ordering;

use clap::Parser;
use fancy_display::FancyDisplay;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{IntoDiagnostic, MietteDiagnostic, WrapErr};
use pep508_rs::{MarkerTree, Requirement};
use pixi_config::ConfigCli;
use pixi_core::{
    WorkspaceLocator,
    diff::LockFileJsonDiff,
    workspace::{MatchSpecs, PypiDeps, WorkspaceMut},
};
use pixi_manifest::{FeatureName, SpecType};
use pixi_pypi_spec::PixiPypiSpec;
use pixi_spec::PixiSpec;
use rattler_conda_types::{MatchSpec, StringMatcher};

use crate::cli_config::{LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig};

/// Checks if there are newer versions of the dependencies and upgrades them in the lockfile and manifest file.
///
/// `pixi upgrade` loosens the requirements for the given packages, updates the lock file and the adapts the manifest accordingly.
#[derive(Parser, Debug, Default)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,
    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    config: ConfigCli,

    #[clap(flatten)]
    pub specs: UpgradeSpecsArgs,

    /// Output the changes in JSON format.
    #[clap(long)]
    pub json: bool,

    /// Only show the changes that would be made, without actually updating the
    /// manifest, lock file, or environment.
    #[clap(short = 'n', long)]
    pub dry_run: bool,
}

#[derive(Parser, Debug, Default)]
pub struct UpgradeSpecsArgs {
    /// The packages to upgrade
    pub packages: Option<Vec<String>>,

    /// The feature to update
    #[clap(long = "feature", short = 'f', default_value_t)]
    pub feature: FeatureName,

    /// The packages which should be excluded
    #[clap(long, conflicts_with = "packages")]
    pub exclude: Option<Vec<String>>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(args.config.clone());

    let mut workspace = workspace.modify()?;

    // Ensure that the given feature exists
    let Some(feature) = workspace
        .workspace()
        .workspace
        .value
        .feature(&args.specs.feature)
    else {
        miette::bail!(
            "could not find a feature named {}",
            args.specs.feature.fancy_display()
        )
    };

    if !args.no_install_config.allow_installs()
        && (args.lock_file_update_config.lock_file_usage.frozen
            || args.lock_file_update_config.lock_file_usage.locked)
    {
        tracing::warn!(
            "using `--frozen` or `--locked` will not make any changes and does not display results. You probably meant: `--dry-run`"
        )
    }

    let (match_specs, pypi_deps) = parse_specs(feature, &args, &workspace)?;
    let (update_deps, workspace) = match workspace
        .update_dependencies(
            match_specs,
            pypi_deps,
            IndexMap::default(),
            args.no_install_config.no_install,
            &args.lock_file_update_config.lock_file_usage()?,
            &args.specs.feature,
            &[],
            false,
            args.dry_run,
        )
        .await
    {
        Ok(update_deps) => (
            update_deps,
            if args.dry_run {
                workspace.revert().await.into_diagnostic()?
            } else {
                workspace.save().await.into_diagnostic()?
            },
        ),
        Err(e) => {
            return Err(e);
        }
    };

    // Is there something to report?
    if let Some(update_deps) = update_deps {
        let diff = update_deps.lock_file_diff;
        // Format as json?
        if args.json {
            let json_diff = LockFileJsonDiff::new(Some(&workspace), diff);
            let json = serde_json::to_string_pretty(&json_diff).expect("failed to convert to json");
            println!("{}", json);
        } else {
            diff.print()
                .into_diagnostic()
                .context("failed to print lock-file diff")?;
        }
    } else {
        eprintln!(
            "{}All packages are already up-to-date",
            console::style(console::Emoji("✔ ", "")).green()
        );
    }

    Ok(())
}

/// Parses the specifications for dependencies from the given feature,
/// arguments, and workspace.
///
/// This function processes the dependencies and PyPi dependencies specified in
/// the feature, filters them based on the provided arguments, and returns the
/// resulting match specifications and PyPi dependencies.
pub fn parse_specs(
    feature: &pixi_manifest::Feature,
    args: &Args,
    workspace: &WorkspaceMut,
) -> miette::Result<(MatchSpecs, PypiDeps)> {
    let spec_type = SpecType::Run;
    let match_spec_iter = feature
        .dependencies(spec_type, None)
        .into_iter()
        .flat_map(|deps| deps.into_owned());
    let pypi_deps_iter = feature
        .pypi_dependencies(None)
        .into_iter()
        .flat_map(|deps| deps.into_owned());
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
        // Don't upgrade excluded packages
        .filter(|(name, _)| match &args.specs.exclude {
            None => true,
            Some(exclude) if exclude.contains(&name.as_normalized().to_string()) => false,
            _ => true,
        })
        // If specific packages have been requested, only upgrade those
        .filter(|(name, _)| match &args.specs.packages {
            None => true,
            Some(packages) if packages.contains(&name.as_normalized().to_string()) => true,
            _ => false,
        })
        // Only upgrade version specs
        .filter_map(|(name, req)| match req {
            PixiSpec::DetailedVersion(version_spec) => {
                let mut nameless_match_spec = version_spec
                    .try_into_nameless_match_spec(&workspace.workspace().channel_config())
                    .ok()?;
                // If it is a detailed spec, always unset version
                nameless_match_spec.version = None;

                // If the package as specifically requested, unset more fields
                if let Some(packages) = &args.specs.packages {
                    if packages.contains(&name.as_normalized().to_string()) {
                        // If the build contains a wildcard, keep it
                        nameless_match_spec.build = match nameless_match_spec.build {
                            Some(
                                build @ StringMatcher::Glob(_) | build @ StringMatcher::Regex(_),
                            ) => Some(build),
                            _ => None,
                        };
                        nameless_match_spec.build_number = None;
                        nameless_match_spec.md5 = None;
                        nameless_match_spec.sha256 = None;
                        // These are still to sensitive to be unset, so skipping
                        // these for now
                        // nameless_match_spec.url = None;
                        // nameless_match_spec.file_name = None;
                        // nameless_match_spec.channel = None;
                        // nameless_match_spec.subdir = None;
                    }
                }

                Some((
                    name.clone(),
                    (
                        MatchSpec::from_nameless(nameless_match_spec, Some(name)),
                        spec_type,
                    ),
                ))
            }
            PixiSpec::Version(_) => Some((name.clone(), (MatchSpec::from(name), spec_type))),
            _ => {
                tracing::debug!("skipping non-version spec {:?}", req);
                None
            }
        })
        // Only upgrade in pyproject.toml if it is explicitly mentioned in
        // `tool.pixi.dependencies.python`
        .filter(|(name, _)| {
            if name.as_normalized() == "python" {
                if let pixi_manifest::ManifestDocument::PyProjectToml(document) =
                    workspace.document()
                {
                    if document
                        .get_nested_table(&["tool", "pixi", "dependencies", "python"])
                        .is_err()
                    {
                        return false;
                    }
                }
            }
            true
        })
        .collect();
    let pypi_deps = pypi_deps_iter
        // Don't upgrade excluded packages
        .filter(|(name, _)| match &args.specs.exclude {
            None => true,
            Some(exclude) if exclude.contains(&name.as_normalized().to_string()) => false,
            _ => true,
        })
        // If specific packages have been requested, only upgrade those
        .filter(|(name, _)| match &args.specs.packages {
            None => true,
            Some(packages) if packages.contains(&name.as_normalized().to_string()) => true,
            _ => false,
        })
        // Only upgrade version specs
        .filter_map(|(name, req)| match &req {
            PixiPypiSpec::Version { extras, .. } => Some((
                name.clone(),
                Requirement {
                    name: name.as_normalized().clone(),
                    extras: extras.clone(),
                    // TODO: Add marker support here to avoid overwriting existing markers
                    marker: MarkerTree::default(),
                    origin: None,
                    version_or_url: None,
                },
                req,
            )),
            PixiPypiSpec::RawVersion(_) => Some((
                name.clone(),
                Requirement {
                    name: name.as_normalized().clone(),
                    extras: Vec::default(),
                    marker: MarkerTree::default(),
                    origin: None,
                    version_or_url: None,
                },
                req,
            )),
            _ => None,
        })
        .map(|(name, req, pixi_req)| {
            let location = workspace.document().pypi_dependency_location(
                &name,
                None, // TODO: add support for platforms
                &args.specs.feature,
            );
            (name, (req, Some(pixi_req), location))
        })
        .collect();

    Ok((match_specs, pypi_deps))
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
