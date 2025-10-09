use std::cmp::Ordering;

use clap::Parser;
use fancy_display::FancyDisplay;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use miette::{IntoDiagnostic, MietteDiagnostic, WrapErr};
use pep508_rs::{MarkerTree, Requirement};
use pixi_config::ConfigCli;
use pixi_core::{
    WorkspaceLocator,
    lock_file::UpdateContext,
    workspace::{MatchSpecs, PypiDeps, WorkspaceMut},
};
use pixi_diff::{LockFileDiff, LockFileJsonDiff};
use pixi_manifest::{FeatureName, SpecType};
use pixi_pypi_spec::PixiPypiSpec;
use pixi_spec::PixiSpec;
use rattler_conda_types::{MatchSpec, Platform, StringMatcher};

use crate::cli_config::{LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig};

/// Checks if there are newer versions of the dependencies and upgrades them in the lockfile and manifest file.
///
/// `pixi upgrade` loosens the requirements for the given packages, updates the lock file and the adapts the manifest accordingly.
/// By default, all features are upgraded.
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
    #[clap(long = "feature", short = 'f')]
    pub feature: Option<FeatureName>,

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

    let features = {
        if let Some(feature_arg) = &args.specs.feature {
            // Ensure that the given feature exists
            let Some(feature) = workspace.workspace().workspace.value.feature(feature_arg) else {
                miette::bail!(
                    "could not find a feature named {}",
                    feature_arg.fancy_display()
                )
            };
            Vec::from([feature.clone()])
        } else {
            workspace
                .workspace()
                .workspace
                .value
                .features
                .clone()
                .into_values()
                .collect()
        }
    };

    if !args.no_install_config.allow_installs()
        && (args.lock_file_update_config.lock_file_usage.frozen
            || args.lock_file_update_config.lock_file_usage.locked)
    {
        tracing::warn!(
            "using `--frozen` or `--locked` will not make any changes and does not display results. You probably meant: `--dry-run`"
        )
    }

    let all_platforms: Vec<Platform> = workspace
        .workspace()
        .workspace
        .value
        .workspace
        .platforms
        .iter()
        .copied()
        .collect();

    if let Some(package_names) = &args.specs.packages {
        let available_packages: Vec<String> = features
            .clone()
            .into_iter()
            .map(|f| collect_available_packages(&f, &all_platforms))
            .fold(IndexSet::new(), |mut acc, set| {
                acc.extend(set);
                acc
            })
            .into_iter()
            .collect();

        for package in package_names {
            ensure_package_exists(package, &available_packages)?;
        }
    }

    let specs_by_feature = features
        .into_iter()
        .map(|f| {
            let specs = collect_specs_by_target(&f, &args, &workspace, &all_platforms)?;
            Ok((f.name.clone(), specs))
        })
        .collect::<miette::Result<SpecsByFeature>>()?;

    let lock_file_usage = args.lock_file_update_config.lock_file_usage()?;

    // Capture original lock-file for combined JSON output (non-dry-run).
    let original_lock_file = workspace.workspace().load_lock_file().await?;

    let mut printed_any = false;

    for (feature_name, specs) in specs_by_feature {
        let SpecsByTarget {
            default_match_specs,
            default_pypi_deps,
            per_platform,
        } = specs;

        if !default_match_specs.is_empty() || !default_pypi_deps.is_empty() {
            if let Some(update) = workspace
                .update_dependencies(
                    default_match_specs,
                    default_pypi_deps,
                    IndexMap::default(),
                    args.no_install_config.no_install,
                    &lock_file_usage,
                    &feature_name,
                    &[],
                    false,
                    args.dry_run,
                )
                .await?
            {
                let diff = update.lock_file_diff;
                if !args.json {
                    diff.print()
                        .into_diagnostic()
                        .context("failed to print lock-file diff")?;
                }
                printed_any = true;
            }
        }

        for (platform, (platform_match_specs, platform_pypi_deps)) in per_platform {
            if platform_match_specs.is_empty() && platform_pypi_deps.is_empty() {
                continue;
            }

            if let Some(update) = workspace
                .update_dependencies(
                    platform_match_specs,
                    platform_pypi_deps,
                    IndexMap::default(),
                    args.no_install_config.no_install,
                    &lock_file_usage,
                    &feature_name,
                    &[platform],
                    false,
                    args.dry_run,
                )
                .await?
            {
                let diff = update.lock_file_diff;
                if !args.json {
                    if printed_any {
                        println!();
                    }
                    diff.print()
                        .into_diagnostic()
                        .context("failed to print lock-file diff")?;
                }
                printed_any = true;
            }
        }
    }

    // If JSON is requested, emit a single combined diff once.
    if args.json {
        if args.dry_run {
            // Compute a combined diff by solving once against the final in-memory manifest
            // without writing to disk, then revert. Reuse the already-loaded original lockfile.
            let derived = UpdateContext::builder(workspace.workspace())
                .with_lock_file(original_lock_file.clone())
                .with_no_install(args.no_install_config.no_install || args.dry_run)
                .finish()
                .await?
                .update()
                .await?;
            let diff = LockFileDiff::from_lock_files(&original_lock_file, &derived.lock_file);
            let json_diff =
                LockFileJsonDiff::new(Some(workspace.workspace().named_environments()), diff);
            let json = serde_json::to_string_pretty(&json_diff).expect("failed to convert to json");
            println!("{}", json);
            // Revert changes after computing the diff in dry-run mode.
            let _ = workspace.revert().await.into_diagnostic()?;
        } else {
            // Reload the resulting lock-file and compute a combined diff against the original.
            let saved_workspace = workspace.save().await.into_diagnostic()?;
            let updated_lock_file = saved_workspace.load_lock_file().await?;
            let diff = LockFileDiff::from_lock_files(&original_lock_file, &updated_lock_file);
            let json_diff = LockFileJsonDiff::new(Some(saved_workspace.named_environments()), diff);
            let json = serde_json::to_string_pretty(&json_diff).expect("failed to convert to json");
            println!("{}", json);
        }
        return Ok(());
    }

    // Persist or revert changes at the end (non-JSON path)
    let _workspace = if args.dry_run {
        workspace.revert().await.into_diagnostic()?
    } else {
        workspace.save().await.into_diagnostic()?
    };

    // Is there something to report?
    if !printed_any {
        eprintln!(
            "{}All packages are already up-to-date",
            console::style(console::Emoji("✔ ", "")).green()
        );
    }

    Ok(())
}

/// A grouping of dependency specs by target table.
struct SpecsByTarget {
    default_match_specs: MatchSpecs,
    default_pypi_deps: PypiDeps,
    per_platform: IndexMap<Platform, (MatchSpecs, PypiDeps)>,
}

type SpecsByFeature = IndexMap<FeatureName, SpecsByTarget>;

/// Collects specs for the default target and for each platform, partitioning
/// out default-owned names so platform targets only get platform-owned entries.
fn collect_specs_by_target(
    feature: &pixi_manifest::Feature,
    args: &Args,
    workspace: &WorkspaceMut,
    platforms: &[Platform],
) -> miette::Result<SpecsByTarget> {
    // Determine default-owned names for partitioning
    let default_deps_names: IndexSet<_> = feature
        .dependencies(SpecType::Run, None)
        .map(|deps| deps.keys().cloned().collect())
        .unwrap_or_default();
    let default_pypi_names: IndexSet<_> = feature
        .pypi_dependencies(None)
        .map(|deps| deps.keys().cloned().collect())
        .unwrap_or_default();

    // Parse default-target specs (written to default location)
    let (default_match_specs, default_pypi_deps) =
        parse_specs_for_platform(feature, args, workspace, None)?;

    // Parse per-platform specs and filter out default-owned names
    let mut per_platform: IndexMap<Platform, (MatchSpecs, PypiDeps)> = IndexMap::new();
    for &platform in platforms {
        let (all_ms, all_py) = parse_specs_for_platform(feature, args, workspace, Some(platform))?;

        let platform_match_specs: MatchSpecs = all_ms
            .into_iter()
            .filter(|(name, _)| !default_deps_names.contains(name))
            .collect();
        let platform_pypi_deps: PypiDeps = all_py
            .into_iter()
            .filter(|(name, _)| !default_pypi_names.contains(name))
            .collect();

        per_platform.insert(platform, (platform_match_specs, platform_pypi_deps));
    }

    Ok(SpecsByTarget {
        default_match_specs,
        default_pypi_deps,
        per_platform,
    })
}

/// Collects available package names (conda run + pypi) across the default
/// and all platform-specific targets, de-duplicated while preserving order.
fn collect_available_packages(
    feature: &pixi_manifest::Feature,
    platforms: &[Platform],
) -> IndexSet<String> {
    let mut available: IndexSet<String> = IndexSet::new();

    // Default target
    if let Some(deps) = feature.dependencies(SpecType::Run, None) {
        for (name, _) in deps.into_owned() {
            available.insert(name.as_normalized().to_string());
        }
    }
    if let Some(deps) = feature.pypi_dependencies(None) {
        for (name, _) in deps.into_owned() {
            available.insert(name.as_normalized().to_string());
        }
    }

    // Platform-specific targets
    for &platform in platforms {
        if let Some(deps) = feature.dependencies(SpecType::Run, Some(platform)) {
            for (name, _) in deps.into_owned() {
                available.insert(name.as_normalized().to_string());
            }
        }
        if let Some(deps) = feature.pypi_dependencies(Some(platform)) {
            for (name, _) in deps.into_owned() {
                available.insert(name.as_normalized().to_string());
            }
        }
    }

    available
}

/// Parses the specifications for dependencies from the given feature,
/// arguments, and workspace.
///
/// This function processes the dependencies and PyPi dependencies specified in
/// the feature, filters them based on the provided arguments, and returns the
/// resulting match specifications and PyPi dependencies.
pub fn parse_specs_for_platform(
    feature: &pixi_manifest::Feature,
    args: &Args,
    workspace: &WorkspaceMut,
    platform: Option<Platform>,
) -> miette::Result<(MatchSpecs, PypiDeps)> {
    let spec_type = SpecType::Run;
    let match_spec_iter = feature
        .dependencies(spec_type, platform)
        .into_iter()
        .flat_map(|deps| deps.into_owned());
    let pypi_deps_iter = feature
        .pypi_dependencies(platform)
        .into_iter()
        .flat_map(|deps| deps.into_owned());
    // Note: package existence is validated across all platforms in `execute`.
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
            let location =
                workspace
                    .document()
                    .pypi_dependency_location(&name, platform, &feature.name);
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
