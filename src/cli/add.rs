use crate::{
    config::ConfigCli,
    environment::{get_up_to_date_prefix, verify_prefix_location_unchanged, LockFileUsage},
    project::{has_features::HasFeatures, DependencyType, Project, SpecType},
    FeatureName,
};
use clap::Parser;
use itertools::{Either, Itertools};

use crate::project::grouped_environment::GroupedEnvironment;
use indexmap::IndexMap;
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::{
    version_spec::{LogicalOperator, RangeOperator},
    Channel, MatchSpec, NamelessMatchSpec, PackageName, ParseStrictness, Platform, Version,
    VersionBumpType, VersionSpec,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{resolvo, SolverImpl};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

/// Adds a dependency to the project
#[derive(Parser, Debug, Default)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specify the dependencies you wish to add to the project.
    ///
    /// The dependencies should be defined as MatchSpec for conda package, or a PyPI requirement
    /// for the --pypi dependencies. If no specific version is provided, the latest version
    /// compatible with your project will be chosen automatically or a * will be used.
    ///
    /// Example usage:
    ///
    /// - `pixi add python=3.9`: This will select the latest minor version that complies with 3.9.*, i.e.,
    ///   python version 3.9.0, 3.9.1, 3.9.2, etc.
    ///
    /// - `pixi add python`: In absence of a specified version, the latest version will be chosen.
    ///   For instance, this could resolve to python version 3.11.3.* at the time of writing.
    ///
    /// Adding multiple dependencies at once is also supported:
    ///
    /// - `pixi add python pytest`: This will add both `python` and `pytest` to the project's dependencies.
    ///
    /// The `--platform` and `--build/--host` flags make the dependency target specific.
    ///
    /// - `pixi add python --platform linux-64 --platform osx-arm64`: Will add the latest version of python for linux-64 and osx-arm64 platforms.
    ///
    /// - `pixi add python --build`: Will add the latest version of python for as a build dependency.
    ///
    /// Mixing `--platform` and `--build`/`--host` flags is supported
    ///
    /// The `--pypi` option will add the package as a pypi-dependency this can not be mixed with the conda dependencies
    /// - `pixi add --pypi boto3`
    /// - `pixi add --pypi "boto3==version"
    ///
    #[arg(required = true)]
    pub specs: Vec<String>,

    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// The specified dependencies are host dependencies. Conflicts with `build` and `pypi`
    #[arg(long, conflicts_with = "build")]
    pub host: bool,

    /// The specified dependencies are build dependencies. Conflicts with `host` and `pypi`
    #[arg(long, conflicts_with = "host")]
    pub build: bool,

    /// The specified dependencies are pypi dependencies. Conflicts with `host` and `build`
    #[arg(long, conflicts_with_all = ["host", "build"])]
    pub pypi: bool,

    /// Don't update lockfile, implies the no-install as well.
    #[clap(long, conflicts_with = "no_install")]
    pub no_lockfile_update: bool,

    /// Don't install the package to the environment, only add the package to the lock-file.
    #[arg(long)]
    pub no_install: bool,

    /// The platform(s) for which the dependency should be added
    #[arg(long, short)]
    pub platform: Vec<Platform>,

    /// The feature for which the dependency should be added
    #[arg(long, short)]
    pub feature: Option<String>,

    #[clap(flatten)]
    pub config: ConfigCli,
}

impl DependencyType {
    pub fn from_args(args: &Args) -> Self {
        if args.pypi {
            Self::PypiDependency
        } else if args.host {
            DependencyType::CondaDependency(SpecType::Host)
        } else if args.build {
            DependencyType::CondaDependency(SpecType::Build)
        } else {
            DependencyType::CondaDependency(SpecType::Run)
        }
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut project = Project::load_or_else_discover(args.manifest_path.as_deref())?
        .with_cli_config(args.config.clone());
    let dependency_type = DependencyType::from_args(&args);
    let spec_platforms = &args.platform;

    // Sanity check of prefix location
    verify_prefix_location_unchanged(project.default_environment().dir().as_path()).await?;

    // Add the platform if it is not already present
    let platforms_to_add = spec_platforms
        .iter()
        .filter(|p| !project.platforms().contains(p))
        .cloned()
        .collect::<Vec<Platform>>();
    project
        .manifest
        .add_platforms(platforms_to_add.iter(), &FeatureName::Default)?;

    let feature_name = args
        .feature
        .map_or(FeatureName::Default, FeatureName::Named);

    match dependency_type {
        DependencyType::CondaDependency(spec_type) => {
            let specs = args
                .specs
                .clone()
                .into_iter()
                .map(|s| MatchSpec::from_str(&s, ParseStrictness::Strict))
                .collect::<Result<Vec<_>, _>>()
                .into_diagnostic()?;
            add_conda_specs_to_project(
                &mut project,
                &feature_name,
                specs,
                spec_type,
                args.no_install,
                args.no_lockfile_update,
                spec_platforms,
            )
            .await
        }
        DependencyType::PypiDependency => {
            // Parse specs as pep508_rs requirements
            let pep508_requirements = args
                .specs
                .clone()
                .into_iter()
                .map(|input| {
                    pep508_rs::Requirement::parse(input.as_ref(), project.root()).into_diagnostic()
                })
                .collect::<miette::Result<Vec<_>>>()?;

            add_pypi_requirements_to_project(
                &mut project,
                &feature_name,
                pep508_requirements,
                spec_platforms,
                args.no_lockfile_update,
                args.no_install,
            )
            .await
        }
    }?;

    for package in args.specs {
        eprintln!(
            "{}Added {}",
            console::style(console::Emoji("✔ ", "")).green(),
            console::style(package).bold(),
        );
    }

    // Print if it is something different from host and dep
    if !matches!(
        dependency_type,
        DependencyType::CondaDependency(SpecType::Run)
    ) {
        eprintln!(
            "Added these as {}.",
            console::style(dependency_type.name()).bold()
        );
    }

    // Print something if we've added for platforms
    if !args.platform.is_empty() {
        eprintln!(
            "Added these only for platform(s): {}",
            console::style(args.platform.iter().join(", ")).bold()
        )
    }

    Project::warn_on_discovered_from_env(args.manifest_path.as_deref());
    Ok(())
}

pub async fn add_pypi_requirements_to_project(
    project: &mut Project,
    feature_name: &FeatureName,
    requirements: Vec<pep508_rs::Requirement>,
    platforms: &[Platform],
    no_update_lockfile: bool,
    no_install: bool,
) -> miette::Result<()> {
    for requirement in &requirements {
        // TODO: Get best version
        // Add the dependency to the project
        if platforms.is_empty() {
            project
                .manifest
                .add_pypi_dependency(requirement, None, feature_name)?;
        } else {
            for platform in platforms.iter() {
                project
                    .manifest
                    .add_pypi_dependency(requirement, Some(*platform), feature_name)?;
            }
        }
    }
    let lock_file_usage = if no_update_lockfile {
        LockFileUsage::Frozen
    } else {
        LockFileUsage::Update
    };

    get_up_to_date_prefix(
        &project.default_environment(),
        lock_file_usage,
        no_install,
        IndexMap::default(),
    )
    .await?;

    project.save()?;

    Ok(())
}

pub async fn add_conda_specs_to_project(
    project: &mut Project,
    feature_name: &FeatureName,
    specs: Vec<MatchSpec>,
    spec_type: SpecType,
    no_install: bool,
    no_update_lockfile: bool,
    specs_platforms: &[Platform],
) -> miette::Result<()> {
    // Split the specs into package name and version specifier
    let new_specs = specs
        .into_iter()
        .map(|spec| match &spec.name {
            Some(name) => Ok((name.clone(), spec.into())),
            None => Err(miette::miette!("missing package name for spec '{spec}'")),
        })
        .collect::<miette::Result<HashMap<PackageName, NamelessMatchSpec>>>()?;

    // Fetch the repodata for the project
    let sparse_repo_data = project.fetch_sparse_repodata().await?;

    // Determine the best version per platform
    let mut package_versions = HashMap::<PackageName, HashSet<Version>>::new();

    // Get the grouped environments that contain the feature
    let grouped_environments: Vec<GroupedEnvironment> = project
        .grouped_environments()
        .iter()
        .filter(|env| {
            env.features()
                .map(|feat| &feat.name)
                .contains(&feature_name)
        })
        .cloned()
        .collect();

    // TODO: show progress of this set of solves
    // TODO: Make this parallel
    // TODO: Make this more efficient by reusing the solves in the get_up_to_date_prefix
    for grouped_environment in grouped_environments {
        let platforms = if specs_platforms.is_empty() {
            Either::Left(grouped_environment.platforms().into_iter())
        } else {
            Either::Right(specs_platforms.iter().copied())
        };

        for platform in platforms {
            // Solve the environment with the new specs added
            let solved_versions = match determine_best_version(
                &grouped_environment,
                &new_specs,
                spec_type,
                &sparse_repo_data,
                platform,
            ) {
                Ok(versions) => versions,
                Err(err) => {
                    return Err(err).wrap_err_with(|| miette::miette!(
                        "could not determine any available versions for {} on {platform}. Either the package could not be found or version constraints on other dependencies result in a conflict.",
                        new_specs.keys().map(|s| s.as_source()).join(", ")
                    ));
                }
            };

            // Collect all the versions seen.
            for (name, version) in solved_versions {
                package_versions.entry(name).or_default().insert(version);
            }
        }
    }

    // Update the specs passed on the command line with the best available versions.
    for (name, spec) in new_specs {
        let updated_spec = if spec.version.is_none() {
            let mut updated_spec = spec.clone();
            if let Some(versions_seen) = package_versions.get(&name).cloned() {
                updated_spec.version = determine_version_constraint(&versions_seen);
            } else {
                updated_spec.version = determine_version_constraint(&determine_latest_versions(
                    project,
                    specs_platforms,
                    &sparse_repo_data,
                    &name,
                )?);
            }
            updated_spec
        } else {
            spec
        };
        let spec = MatchSpec::from_nameless(updated_spec, Some(name));

        // Add the dependency to the project
        if specs_platforms.is_empty() {
            project
                .manifest
                .add_dependency(&spec, spec_type, None, feature_name)?;
        } else {
            for platform in specs_platforms.iter() {
                project
                    .manifest
                    .add_dependency(&spec, spec_type, Some(*platform), feature_name)?;
            }
        }
    }
    let lock_file_usage = if no_update_lockfile {
        LockFileUsage::Frozen
    } else {
        LockFileUsage::Update
    };

    // Update the prefix
    get_up_to_date_prefix(
        &project.default_environment(),
        lock_file_usage,
        no_install,
        sparse_repo_data,
    )
    .await?;

    project.save()?;

    Ok(())
}

/// Get all the latest versions found in the platforms repodata.
fn determine_latest_versions(
    project: &Project,
    platforms: &[Platform],
    sparse_repo_data: &IndexMap<(Channel, Platform), SparseRepoData>,
    name: &PackageName,
) -> miette::Result<Vec<Version>> {
    // If we didn't find any versions, we'll just use the latest version we can find in the repodata.
    let mut found_records = Vec::new();

    // Get platforms to search for including NoArch
    let platforms = if platforms.is_empty() {
        let mut temp = project.platforms().into_iter().collect_vec();
        temp.push(Platform::NoArch);
        temp
    } else {
        let mut temp = platforms.to_vec();
        temp.push(Platform::NoArch);
        temp
    };

    // Search for the package in the all the channels and platforms
    for channel in project.channels() {
        for platform in &platforms {
            let sparse_repo_data = sparse_repo_data.get(&(channel.clone(), *platform));
            if let Some(sparse_repo_data) = sparse_repo_data {
                let records = sparse_repo_data.load_records(name).into_diagnostic()?;
                // Add max of every channel and platform
                if let Some(max_record) = records
                    .into_iter()
                    .max_by_key(|record| record.package_record.version.version().clone())
                {
                    found_records.push(max_record);
                }
            };
        }
    }

    // Determine the version constraint based on the max of every channel and platform.
    Ok(found_records
        .iter()
        .map(|record| record.package_record.version.version().clone())
        .collect_vec())
}
/// Given several specs determines the highest installable version for them.
pub fn determine_best_version(
    environment: &GroupedEnvironment,
    new_specs: &HashMap<PackageName, NamelessMatchSpec>,
    new_specs_type: SpecType,
    sparse_repo_data: &IndexMap<(Channel, Platform), SparseRepoData>,
    platform: Platform,
) -> miette::Result<HashMap<PackageName, Version>> {
    // Build the combined set of specs while updating the dependencies with the new specs.
    let dependencies = SpecType::all()
        .map(|spec_type| {
            let mut deps = environment.dependencies(Some(spec_type), Some(platform));
            if spec_type == new_specs_type {
                for (new_name, new_spec) in new_specs.iter() {
                    deps.remove(new_name); // Remove any existing specs
                    deps.insert(new_name.clone(), new_spec.clone()); // Add the new specs
                }
            }
            deps
        })
        .reduce(|acc, deps| acc.overwrite(&deps))
        .unwrap_or_default();

    // Extract the package names from all the dependencies
    let package_names = dependencies.names().cloned().collect_vec();

    // Get the repodata for the current platform and for NoArch
    let platform_sparse_repo_data = environment
        .channels()
        .into_iter()
        .cloned()
        .cartesian_product(vec![platform, Platform::NoArch])
        .filter_map(|target| sparse_repo_data.get(&target));

    // Load only records we need for this platform
    let available_packages = SparseRepoData::load_records_recursive(
        platform_sparse_repo_data,
        package_names.iter().cloned(),
        None,
    )
    .into_diagnostic()?;

    // Construct a solver task to start solving.
    let task = rattler_solve::SolverTask {
        specs: dependencies
            .iter_specs()
            .map(|(name, spec)| MatchSpec::from_nameless(spec.clone(), Some(name.clone())))
            .collect(),

        available_packages: &available_packages,

        virtual_packages: environment.virtual_packages(platform),

        locked_packages: vec![],

        pinned_packages: vec![],

        timeout: None,
    };

    let records = resolvo::Solver.solve(task).into_diagnostic()?;

    // Determine the versions of the new packages
    Ok(records
        .into_iter()
        .filter(|record| new_specs.contains_key(&record.package_record.name))
        .map(|record| {
            (
                record.package_record.name,
                record.package_record.version.into(),
            )
        })
        .collect())
}

/// Given a set of versions, determines the best version constraint to use that captures all of them.
fn determine_version_constraint<'a>(
    versions: impl IntoIterator<Item = &'a Version>,
) -> Option<VersionSpec> {
    let (min_version, max_version) = versions.into_iter().minmax().into_option()?;
    let lower_bound = min_version.clone();
    let upper_bound = max_version
        .pop_segments(1)
        .unwrap_or_else(|| max_version.clone())
        .bump(VersionBumpType::Last)
        .ok()?;
    Some(VersionSpec::Group(
        LogicalOperator::And,
        vec![
            VersionSpec::Range(RangeOperator::GreaterEquals, lower_bound),
            VersionSpec::Range(RangeOperator::Less, upper_bound),
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_version_constraint() {
        insta::assert_snapshot!(determine_version_constraint(&["1.2.0".parse().unwrap()])
            .unwrap()
            .to_string(), @">=1.2.0,<1.3");

        insta::assert_snapshot!(determine_version_constraint(&["1.2.0".parse().unwrap(), "1.3.0".parse().unwrap()])
            .unwrap()
            .to_string(), @">=1.2.0,<1.4");
    }
}
