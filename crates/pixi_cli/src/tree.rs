use crate::cli_config::{LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig};
use crate::shared::tree::{
    Dependency, Package, PackageSource, build_reverse_dependency_map, print_dependency_tree,
    print_inverted_dependency_tree,
};
use ahash::HashSet;
use clap::Parser;
use console::Color;
use fancy_display::FancyDisplay;
use miette::WrapErr;
use pep508_rs::{ExtraName, MarkerEnvironment, Requirement};
use pixi_core::workspace::Environment;
use pixi_core::{WorkspaceLocator, lock_file::UpdateLockFileOptions};
use pixi_manifest::FeaturesExt;
use pixi_uv_conversions::to_marker_environment;
use pypi_modifiers::pypi_marker_env::determine_marker_environment;
use rattler_conda_types::{PackageName, Platform};
use rattler_lock::{LockedPackage, PypiPackageData};
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

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let environment = workspace
        .environment_from_name_or_env_var(args.environment)
        .wrap_err("Environment not found")?;

    let lock_file = workspace
        .update_lock_file(
            Some(pixi_reporters::TopLevelProgress::from_global()),
            UpdateLockFileOptions {
                lock_file_usage: args.lock_file_update_config.lock_file_usage()?,
                no_install: args.no_install_config.no_install,
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
                ..Default::default()
            },
        )
        .await
        .wrap_err("Failed to update lock file")?
        .0
        .into_lock_file();

    let platform = args.platform.unwrap_or_else(|| environment.best_platform());
    let locked_deps = lock_file
        .environment(environment.name().as_str())
        .and_then(|env| {
            let p = lock_file.platform(&platform.to_string())?;
            env.packages(p).map(Vec::from_iter)
        })
        .unwrap_or_default();

    let dep_map = DependencyMapBuilder::new(&environment, platform, &locked_deps).build();
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

/// Per-package activated PyPI extras, propagated transitively from the
/// manifest direct deps through `requires_dist` to a fixed point.
pub(crate) struct PypiExtrasResolver {
    marker_env: Option<MarkerEnvironment>,
    activated: HashMap<String, Vec<ExtraName>>,
}

impl PypiExtrasResolver {
    pub fn new(
        environment: &Environment<'_>,
        platform: Platform,
        locked_deps: &[&LockedPackage],
    ) -> Self {
        let marker_env = Self::build_marker_env(locked_deps, platform);
        let activated = Self::propagate(environment, platform, locked_deps, marker_env.as_ref());
        Self {
            marker_env,
            activated,
        }
    }

    pub fn extras_for(&self, name: &str) -> &[ExtraName] {
        self.activated
            .get(name)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Falls back to `marker.is_true()` when no Python is locked.
    pub fn is_active(&self, req: &Requirement, parent_extras: &[ExtraName]) -> bool {
        match &self.marker_env {
            Some(env) => req.evaluate_markers(env, parent_extras),
            None => req.marker.is_true(),
        }
    }

    /// Parent extras whose presence is required for `req`'s marker to pass.
    /// Empty for base dependencies.
    pub fn activating_extras<'a>(
        &'a self,
        req: &'a Requirement,
        parent_extras: &'a [ExtraName],
    ) -> impl Iterator<Item = &'a ExtraName> + 'a {
        let gating_env = self
            .marker_env
            .as_ref()
            .filter(|env| !req.marker.is_true() && !req.evaluate_markers(env, &[]));
        parent_extras.iter().filter(move |e| {
            gating_env.is_some_and(|env| req.evaluate_markers(env, std::slice::from_ref(*e)))
        })
    }

    fn build_marker_env(
        locked_deps: &[&LockedPackage],
        platform: Platform,
    ) -> Option<MarkerEnvironment> {
        let python_record = locked_deps
            .iter()
            .filter_map(|p| p.as_conda())
            .find(|c| c.name().as_normalized() == "python")
            .and_then(|c| c.record())?;
        let uv_env = determine_marker_environment(platform, python_record).ok()?;
        to_marker_environment(&uv_env).ok()
    }

    fn propagate(
        environment: &Environment<'_>,
        platform: Platform,
        locked_deps: &[&LockedPackage],
        marker_env: Option<&MarkerEnvironment>,
    ) -> HashMap<String, Vec<ExtraName>> {
        let pypi_by_name: HashMap<String, &PypiPackageData> = locked_deps
            .iter()
            .filter_map(|p| p.as_pypi())
            .map(|p| (p.name().as_dist_info_name().into_owned(), p))
            .collect();

        // Pre-populate so the propagation loop can use `get_mut(&str)` and
        // skip allocating an owned key per requirement.
        let mut activated: HashMap<String, Vec<ExtraName>> = pypi_by_name
            .keys()
            .map(|name| (name.clone(), Vec::new()))
            .collect();

        for (name, specs) in environment.pypi_dependencies(Some(platform)) {
            let key = name.as_normalized().as_dist_info_name();
            if let Some(entry) = activated.get_mut(key.as_ref()) {
                for spec in specs {
                    for e in spec.extras() {
                        if !entry.contains(e) {
                            entry.push(e.clone());
                        }
                    }
                }
            }
        }

        loop {
            let mut changed = false;
            for (name, pkg) in &pypi_by_name {
                let extras_snapshot: Vec<ExtraName> =
                    activated.get(name).cloned().unwrap_or_default();
                for req in pkg.requires_dist() {
                    let active = match marker_env {
                        Some(env) => req.evaluate_markers(env, &extras_snapshot),
                        None => req.marker.is_true(),
                    };
                    if !active {
                        continue;
                    }
                    let target = req.name.as_dist_info_name();
                    let Some(entry) = activated.get_mut(target.as_ref()) else {
                        continue;
                    };
                    for e in &req.extras {
                        if !entry.contains(e) {
                            entry.push(e.clone());
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }

        activated
    }
}

/// Turns a list of locked packages into the dependency map consumed by the
/// renderer. Uses [`PypiExtrasResolver`] to evaluate PyPI markers with the
/// correct activated-extras context.
pub(crate) struct DependencyMapBuilder<'a> {
    locked_deps: &'a [&'a LockedPackage],
    resolver: PypiExtrasResolver,
}

impl<'a> DependencyMapBuilder<'a> {
    pub fn new(
        environment: &Environment<'_>,
        platform: Platform,
        locked_deps: &'a [&'a LockedPackage],
    ) -> Self {
        Self {
            locked_deps,
            resolver: PypiExtrasResolver::new(environment, platform, locked_deps),
        }
    }

    pub fn build(self) -> HashMap<String, Package> {
        let mut out = HashMap::new();
        for &package in self.locked_deps {
            let Some((name, source, edges)) = self.package_view(package) else {
                continue;
            };
            let version = match package {
                LockedPackage::Conda(c) => c
                    .record()
                    .map(|r| r.version.to_string())
                    .unwrap_or_default(),
                LockedPackage::Pypi(p) => p.version_string(),
            };
            let dependencies =
                Self::merge_edges(edges.into_iter().filter(|e| !e.name.starts_with("__")));
            out.insert(
                name.clone(),
                Package {
                    name,
                    version,
                    dependencies,
                    needed_by: Vec::new(),
                    source,
                },
            );
        }
        out
    }

    fn package_view(
        &self,
        package: &LockedPackage,
    ) -> Option<(String, PackageSource, Vec<Dependency>)> {
        if let Some(conda) = package.as_conda() {
            let name = conda.name().as_normalized().to_string();
            let edges: Vec<Dependency> = conda
                .depends()
                .iter()
                .map(|d| Dependency {
                    name: PackageName::from_matchspec_str_unchecked(d)
                        .as_normalized()
                        .to_string(),
                    via_extras: Vec::new(),
                })
                .collect();
            Some((name, PackageSource::Conda, edges))
        } else if let Some(pypi) = package.as_pypi() {
            let name = pypi.name().as_dist_info_name().into_owned();
            let parent_extras = self.resolver.extras_for(&name);
            let edges = pypi
                .requires_dist()
                .iter()
                .filter_map(|req| {
                    if !self.resolver.is_active(req, parent_extras) {
                        tracing::info!(
                            "Skipping {} specified by {} due to marker {:?}",
                            req.name,
                            name,
                            req.marker
                        );
                        return None;
                    }
                    Some(Dependency {
                        name: req.name.as_dist_info_name().into_owned(),
                        via_extras: self
                            .resolver
                            .activating_extras(req, parent_extras)
                            .map(ToString::to_string)
                            .collect(),
                    })
                })
                .collect();
            Some((name, PackageSource::Pypi, edges))
        } else {
            None
        }
    }

    /// Dedup by name, preserving `requires_dist` order. Base edges win over
    /// extra-gated duplicates; extras are unioned otherwise.
    fn merge_edges<I: IntoIterator<Item = Dependency>>(edges: I) -> Vec<Dependency> {
        let mut by_name: indexmap::IndexMap<String, Dependency> = indexmap::IndexMap::new();
        for edge in edges {
            if let Some(existing) = by_name.get_mut(&edge.name) {
                if existing.via_extras.is_empty() || edge.via_extras.is_empty() {
                    existing.via_extras.clear();
                    continue;
                }
                for e in edge.via_extras {
                    if !existing.via_extras.contains(&e) {
                        existing.via_extras.push(e);
                    }
                }
            } else {
                by_name.insert(edge.name.clone(), edge);
            }
        }
        by_name.into_values().collect()
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_core::Workspace;
    use rattler_lock::LockFile;

    /// Render the PyPI subset of an example workspace through the production
    /// printer into a buffer for snapshot comparison.
    fn render_pypi_tree(manifest: &std::path::Path, platform: Platform) -> String {
        console::set_colors_enabled(false);

        let workspace = Workspace::from_path(manifest).unwrap();
        let environment = workspace.default_environment();
        let lock_file = LockFile::from_path(&workspace.lock_file_path()).unwrap();
        let pkgs: Vec<&LockedPackage> = lock_file
            .environment(environment.name().as_str())
            .and_then(|env| {
                let p = lock_file.platform(&platform.to_string())?;
                env.packages(p).map(Vec::from_iter)
            })
            .unwrap_or_default();

        let dep_map: HashMap<String, Package> =
            DependencyMapBuilder::new(&environment, platform, &pkgs)
                .build()
                .into_iter()
                .filter(|(_, p)| p.source == PackageSource::Pypi)
                .collect();
        let mut direct_deps = direct_dependencies(&environment, &platform, &dep_map);
        direct_deps.retain(|n| dep_map.contains_key(n));

        let mut buf: Vec<u8> = Vec::new();
        print_dependency_tree(&mut buf, &dep_map, &direct_deps, &None).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn pypi_extras_tree_snapshots() {
        insta::glob!(
            concat!(env!("CARGO_WORKSPACE_DIR"), "/examples"),
            "{editable-with-extras,pypi}/pixi.toml",
            |manifest| {
                insta::assert_snapshot!(render_pypi_tree(manifest, Platform::Linux64));
            }
        );
    }
}
