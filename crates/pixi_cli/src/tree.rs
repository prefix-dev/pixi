use crate::cli_config::{LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig};
use crate::shared::tree::{
    Package, PackageSource, build_reverse_dependency_map, print_dependency_tree,
    print_inverted_dependency_tree,
};
use ahash::HashSet;
use clap::Parser;
use console::Color;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::WrapErr;
use pixi_core::workspace::Environment;
use pixi_core::{WorkspaceLocator, lock_file::UpdateLockFileOptions};
use pixi_manifest::FeaturesExt;
use rattler_conda_types::Platform;
use rattler_lock::LockedPackageRef;
use std::collections::HashMap;

/// Show a tree of workspace dependencies
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false, long_about = format!(
    "\
    Show a tree of workspace dependencies\n\
    \n\
    Dependency names highlighted in {} are directly specified in the manifest. \
    {} version numbers are conda packages, PyPI version numbers are {}.
    ",
    console::style("green").fg(Color::Green).bold(),
    console::style("Yellow").fg(Color::Yellow),
    console::style("blue").fg(Color::Blue)
))]
pub struct Args {
    /// List only packages matching a regular expression
    #[arg()]
    pub regex: Option<String>,

    /// The platform to list packages for. Defaults to the current platform.
    #[arg(long, short)]
    pub platform: Option<Platform>,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The environment to list packages for. Defaults to the default
    /// environment.
    #[arg(short, long)]
    pub environment: Option<String>,

    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,

    /// Invert tree and show what depends on given package in the regex argument
    #[arg(short, long, requires = "regex")]
    pub invert: bool,
}

/// Simplified package information extracted from the lock file
pub struct PackageInfo {
    name: String,
    dependencies: Vec<String>,
    source: PackageSource,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let environment = workspace
        .environment_from_name_or_env_var(args.environment)
        .wrap_err("Environment not found")?;

    let lock_file = workspace
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: args.lock_file_update_config.lock_file_usage()?,
            no_install: args.no_install_config.no_install,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        })
        .await
        .wrap_err("Failed to update lock file")?
        .0
        .into_lock_file();

    let platform = args.platform.unwrap_or_else(|| environment.best_platform());
    let locked_deps = lock_file
        .environment(environment.name().as_str())
        .and_then(|env| env.packages(platform).map(Vec::from_iter))
        .unwrap_or_default();

    let dep_map = generate_dependency_map(&locked_deps);

    let direct_deps = direct_dependencies(&environment, &platform, &dep_map);

    if !environment.is_default() {
        eprintln!("Environment: {}", environment.name().fancy_display());
    }

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    if args.invert {
        print_inverted_dependency_tree(
            &mut handle,
            &build_reverse_dependency_map(&dep_map),
            &direct_deps,
            &args.regex,
        )
        .wrap_err("Couldn't print the inverted dependency tree")?;
    } else {
        print_dependency_tree(&mut handle, &dep_map, &direct_deps, &args.regex)
            .wrap_err("Couldn't print the dependency tree")?;
    }
    Ok(())
}

/// Helper function to extract package information from a package reference obtained from a lock file.
pub(crate) fn extract_package_info(
    package: rattler_lock::LockedPackageRef<'_>,
) -> Option<PackageInfo> {
    if let Some(conda_package) = package.as_conda() {
        let name = conda_package.record().name.as_normalized().to_string();

        let dependencies: Vec<String> = conda_package
            .record()
            .depends
            .iter()
            .map(|d| {
                d.split_once(' ')
                    .map_or_else(|| d.to_string(), |(dep_name, _)| dep_name.to_string())
            })
            .collect();

        Some(PackageInfo {
            name,
            dependencies,
            source: PackageSource::Conda,
        })
    } else if let Some((pypi_package_data, _pypi_env_data)) = package.as_pypi() {
        let name = pypi_package_data.name.as_dist_info_name().into_owned();
        let dependencies = pypi_package_data
            .requires_dist
            .iter()
            .filter_map(|p| {
                if p.marker.is_true() {
                    Some(p.name.as_dist_info_name().into_owned())
                } else {
                    tracing::info!(
                        "Skipping {} specified by {} due to marker {:?}",
                        p.name,
                        name,
                        p.marker
                    );
                    None
                }
            })
            .collect();

        Some(PackageInfo {
            name,
            dependencies,
            source: PackageSource::Pypi,
        })
    } else {
        None
    }
}

/// Generate a map of dependencies from a list of locked packages.
pub fn generate_dependency_map(locked_deps: &[LockedPackageRef<'_>]) -> HashMap<String, Package> {
    let mut package_dependencies_map = HashMap::new();

    for &package in locked_deps {
        if let Some(package_info) = extract_package_info(package) {
            package_dependencies_map.insert(
                package_info.name.clone(),
                Package {
                    name: package_info.name,
                    version: match package {
                        LockedPackageRef::Conda(conda_data) => {
                            conda_data.record().version.to_string()
                        }
                        LockedPackageRef::Pypi(pypi_data, _) => pypi_data.version.to_string(),
                    },
                    dependencies: package_info
                        .dependencies
                        .into_iter()
                        .filter(|pkg| !pkg.starts_with("__"))
                        .unique()
                        .collect(),
                    needed_by: Vec::new(),
                    source: package_info.source,
                },
            );
        }
    }
    package_dependencies_map
}

/// Extract the direct Conda and PyPI dependencies from the environment
pub fn direct_dependencies(
    environment: &Environment<'_>,
    platform: &Platform,
    dep_map: &HashMap<String, Package>,
) -> HashSet<String> {
    let mut project_dependency_names = environment
        .combined_dependencies(Some(*platform))
        .names()
        .filter(|p| {
            if let Some(value) = dep_map.get(p.as_source()) {
                value.source == PackageSource::Conda
            } else {
                false
            }
        })
        .map(|p| p.as_source().to_string())
        .collect::<HashSet<_>>();

    project_dependency_names.extend(
        environment
            .pypi_dependencies(Some(*platform))
            .into_iter()
            .filter(|(name, _)| {
                if let Some(value) = dep_map.get(&*name.as_normalized().as_dist_info_name()) {
                    value.source == PackageSource::Pypi
                } else {
                    false
                }
            })
            .map(|(name, _)| name.as_normalized().as_dist_info_name().into_owned()),
    );
    project_dependency_names
}
