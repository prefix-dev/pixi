use std::cmp::Ordering;

use clap::Parser;
use fancy_display::FancyDisplay;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use miette::{IntoDiagnostic, MietteDiagnostic, WrapErr};
use pep508_rs::Requirement;
use pixi_config::ConfigCli;
use pixi_core::{
    WorkspaceLocator,
    lock_file::UpdateContext,
    workspace::{MatchSpecs, PypiDeps, WorkspaceMut},
};
use pixi_diff::{LockFileDiff, LockFileJsonDiff};
use pixi_manifest::{FeatureName, PixiPlatform, SpecType, TargetSelector, WorkspaceTarget};
use pixi_pypi_spec::{PixiPypiSource, PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use rattler_conda_types::{MatchSpec, PackageName, StringMatcher};

use crate::cli_config::{LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig};

/// Checks if there are newer versions of the dependencies and upgrades them in the lock file and manifest file.
///
/// `pixi upgrade` loosens the requirements for the given packages, updates the lock file and the adapts the manifest accordingly.
/// By default, all features are upgraded.
#[derive(Parser, Debug, Default)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,
    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    pub config: ConfigCli,

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
        .with_global_config_source(args.config_source.source())
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

    if let Some(package_names) = &args.specs.packages {
        let available_packages: Vec<String> = features
            .clone()
            .into_iter()
            .map(|f| collect_available_packages(&f))
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
            let specs = collect_specs_by_target(&f, &args, &workspace)?;
            Ok((f.name.clone(), specs))
        })
        .collect::<miette::Result<SpecsByFeature>>()?;

    let lock_file_usage = args.lock_file_update_config.lock_file_usage()?;

    // Capture original lock file for combined JSON output (non-dry-run).
    let original_lock_file = workspace
        .workspace()
        .load_lock_file()
        .await?
        .into_lock_file_or_empty_with_warning();

    let mut printed_any = false;

    for (feature_name, specs) in specs_by_feature {
        let SpecsByTarget {
            default_match_specs,
            default_pypi_deps,
            per_target,
        } = specs;

        if (!default_match_specs.is_empty() || !default_pypi_deps.is_empty())
            && let Some(update) = workspace
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
                    .context("failed to print lock file diff")?;
            }
            printed_any = true;
        }

        for (target, (target_match_specs, target_pypi_deps)) in per_target {
            if target_match_specs.is_empty() && target_pypi_deps.is_empty() {
                continue;
            }

            if let Some(update) = workspace
                .update_dependencies(
                    target_match_specs,
                    target_pypi_deps,
                    IndexMap::default(),
                    args.no_install_config.no_install,
                    &lock_file_usage,
                    &feature_name,
                    std::slice::from_ref(&target),
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
                        .context("failed to print lock file diff")?;
                }
                printed_any = true;
            }
        }
    }

    // If JSON is requested, emit a single combined diff once.
    if args.json {
        if args.dry_run {
            // Compute a combined diff by solving once against the final in-memory manifest
            // without writing to disk, then revert. Reuse the already-loaded original lock file.
            let progress = pixi_reporters::TopLevelProgress::from_global();
            let dispatcher = progress
                .clone()
                .register_with(workspace.workspace().command_dispatcher_builder()?)
                .finish();
            let derived = UpdateContext::builder(workspace.workspace(), dispatcher)?
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
            println!("{json}");
            // Revert changes after computing the diff in dry-run mode.
            let _ = workspace.revert().await.into_diagnostic()?;
        } else {
            // Reload the resulting lock file and compute a combined diff against the original.
            // Use the silent version here since we already warned on the first load (line 144).
            let saved_workspace = workspace.save().await.into_diagnostic()?;
            let updated_lock_file = saved_workspace
                .load_lock_file()
                .await?
                .into_lock_file_or_empty();
            let diff = LockFileDiff::from_lock_files(&original_lock_file, &updated_lock_file);
            let json_diff = LockFileJsonDiff::new(Some(saved_workspace.named_environments()), diff);
            let json = serde_json::to_string_pretty(&json_diff).expect("failed to convert to json");
            println!("{json}");
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
    per_target: IndexMap<TargetSelector, (MatchSpecs, PypiDeps)>,
}

type SpecsByFeature = IndexMap<FeatureName, SpecsByTarget>;

/// Collects specs for the default target and for each declared target table,
/// partitioning out default-owned names so target tables only get their own
/// entries. Specs are written back to the selector they were declared under,
/// not to the platforms that selector happens to match.
fn collect_specs_by_target(
    feature: &pixi_manifest::Feature,
    args: &Args,
    workspace: &WorkspaceMut,
) -> miette::Result<SpecsByTarget> {
    // Determine default-owned names for partitioning
    let default_deps_names: IndexSet<_> = feature
        .dependencies(SpecType::Run, None)
        .map(|deps| deps.names().cloned().collect())
        .unwrap_or_default();
    let default_pypi_names: IndexSet<_> = feature
        .pypi_dependencies(None)
        .map(|deps| deps.names().cloned().collect())
        .unwrap_or_default();

    // Parse default-target specs (written to default location)
    let (default_match_specs, default_pypi_deps) =
        parse_specs_for_platform(feature, args, workspace, None)?;

    // Parse each declared target's own specs and filter out default-owned names
    let mut per_target: IndexMap<TargetSelector, (MatchSpecs, PypiDeps)> = IndexMap::new();
    for (selector, target) in feature.targets.user_defined_targets() {
        let (all_ms, all_py) = parse_specs_for_target(feature, args, workspace, selector, target)?;

        let target_match_specs: MatchSpecs = all_ms
            .into_iter()
            .filter(|(name, _)| !default_deps_names.contains(name))
            .collect();
        let target_pypi_deps: PypiDeps = all_py
            .into_iter()
            .filter(|(name, _)| !default_pypi_names.contains(name))
            .collect();

        per_target.insert(selector.clone(), (target_match_specs, target_pypi_deps));
    }

    Ok(SpecsByTarget {
        default_match_specs,
        default_pypi_deps,
        per_target,
    })
}

/// Collects available package names (conda run + pypi) across the default
/// and all declared target tables, de-duplicated while preserving order.
fn collect_available_packages(feature: &pixi_manifest::Feature) -> IndexSet<String> {
    let mut available: IndexSet<String> = IndexSet::new();

    for target in feature.targets.targets() {
        if let Some(deps) = target.dependencies(SpecType::Run) {
            for name in deps.names() {
                available.insert(name.as_normalized().to_string());
            }
        }
        if let Some(deps) = &target.pypi_dependencies {
            for name in deps.names() {
                available.insert(name.as_normalized().to_string());
            }
        }
    }

    available
}

/// Parses the upgradable specs of the default target (`platform` = `None`) or
/// of the target whose selector matches `platform`, resolving across the less
/// specific targets that also match.
pub fn parse_specs_for_platform(
    feature: &pixi_manifest::Feature,
    args: &Args,
    workspace: &WorkspaceMut,
    platform: Option<&PixiPlatform>,
) -> miette::Result<(MatchSpecs, PypiDeps)> {
    let match_spec_iter = feature
        .dependencies(SpecType::Run, platform)
        .into_iter()
        .flat_map(|deps| deps.into_owned().into_specs());
    let pypi_deps_iter = feature
        .pypi_dependencies(platform)
        .into_iter()
        .flat_map(|deps| deps.into_owned().into_specs());
    let target = platform.map(PixiPlatform::as_target_selector);
    parse_specs(
        match_spec_iter,
        pypi_deps_iter,
        args,
        workspace,
        feature,
        target.as_ref(),
    )
}

/// Parses the upgradable specs declared directly on a target table, so the
/// upgraded specs are written back to the same selector.
fn parse_specs_for_target(
    feature: &pixi_manifest::Feature,
    args: &Args,
    workspace: &WorkspaceMut,
    selector: &TargetSelector,
    target: &WorkspaceTarget,
) -> miette::Result<(MatchSpecs, PypiDeps)> {
    let match_spec_iter = target
        .dependencies(SpecType::Run)
        .cloned()
        .into_iter()
        .flat_map(|deps| deps.into_specs());
    let pypi_deps_iter = target
        .pypi_dependencies
        .clone()
        .into_iter()
        .flat_map(|deps| deps.into_specs());
    parse_specs(
        match_spec_iter,
        pypi_deps_iter,
        args,
        workspace,
        feature,
        Some(selector),
    )
}

/// Filters a target's dependencies down to the ones `pixi upgrade` should
/// loosen, building the conda match specs and pypi requirements to update.
fn parse_specs(
    match_spec_iter: impl Iterator<Item = (PackageName, PixiSpec)>,
    pypi_deps_iter: impl Iterator<Item = (PypiPackageName, PixiPypiSpec)>,
    args: &Args,
    workspace: &WorkspaceMut,
    feature: &pixi_manifest::Feature,
    target: Option<&TargetSelector>,
) -> miette::Result<(MatchSpecs, PypiDeps)> {
    let spec_type = SpecType::Run;
    // Note: package existence is validated across all targets in `execute`.
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
            PixiSpec::Version(_) => {
                // A bare version spec carries no extra selectors, so upgrading
                // means dropping the constraint entirely.
                Some((name.clone(), (MatchSpec::from(name), spec_type)))
            }
            PixiSpec::DetailedVersion(detailed) => {
                let mut nameless_match_spec = detailed
                    .try_into_nameless_match_spec(&workspace.workspace().channel_config())
                    .ok()?;
                // If it is a detailed spec, always unset version
                nameless_match_spec.version = None;

                // If the package as specifically requested, unset more fields
                if let Some(packages) = &args.specs.packages
                    && packages.contains(&name.as_normalized().to_string())
                {
                    // If the build contains a wildcard, keep it
                    nameless_match_spec.build = match nameless_match_spec.build {
                        Some(build @ StringMatcher::Glob(_) | build @ StringMatcher::Regex(_)) => {
                            Some(build)
                        }
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

                Some((
                    name.clone(),
                    (
                        MatchSpec::from_nameless(nameless_match_spec, name.clone().into()),
                        spec_type,
                    ),
                ))
            }
            _ => {
                tracing::debug!("skipping non-version spec {:?}", req);
                None
            }
        })
        // Only upgrade in pyproject.toml if it is explicitly mentioned in
        // `tool.pixi.dependencies.python`
        .filter(|(name, _)| {
            if name.as_normalized() == "python"
                && let pixi_manifest::ManifestDocument::PyProjectToml(document) =
                    workspace.document()
                && document
                    .get_nested_table(&["tool", "pixi", "dependencies", "python"])
                    .is_err()
            {
                return false;
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
        // Only upgrade version specs (Registry sources)
        .filter_map(|(name, req)| match &req.source {
            PixiPypiSource::Registry { .. } => Some((
                name.clone(),
                Requirement {
                    name: name.as_normalized().clone(),
                    extras: req.extras.clone(),
                    marker: req.env_markers.clone(),
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
                    .pypi_dependency_location(&name, target, &feature.name);
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
