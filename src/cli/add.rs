use crate::{
    config::ConfigCli,
    environment::{get_up_to_date_prefix, verify_prefix_location_unchanged, LockFileUsage},
    project::{has_features::HasFeatures, DependencyType, Project, SpecType},
    FeatureName,
};
use clap::Parser;
use itertools::{Either, Itertools};

use crate::project::grouped_environment::GroupedEnvironment;
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::{
    version_spec::{LogicalOperator, RangeOperator},
    Channel, MatchSpec, NamelessMatchSpec, PackageName, ParseStrictness, Platform, Version,
    VersionBumpType, VersionSpec,
};
use rattler_repodata_gateway::{Gateway, RepoData};
use rattler_solve::{resolvo, ChannelPriority, RepoDataIter, SolverImpl};
use std::time::Instant;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

/// Adds dependencies to the project
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
    /// - `pixi add python`: In absence of a specified version, the latest version will be chosen.
    ///   For instance, this could resolve to python version 3.11.3.* at the time of writing.
    ///
    /// Adding multiple dependencies at once is also supported:
    /// - `pixi add python pytest`: This will add both `python` and `pytest` to the project's dependencies.
    ///
    /// The `--platform` and `--build/--host` flags make the dependency target specific.
    /// - `pixi add python --platform linux-64 --platform osx-arm64`: Will add the latest version of python for linux-64 and osx-arm64 platforms.
    /// - `pixi add python --build`: Will add the latest version of python for as a build dependency.
    ///
    /// Mixing `--platform` and `--build`/`--host` flags is supported
    ///
    /// The `--pypi` option will add the package as a pypi dependency. This can not be mixed with the conda dependencies
    /// - `pixi add --pypi boto3`
    /// - `pixi add --pypi "boto3==version"
    ///
    /// If the project manifest is a `pyproject.toml`, adding a pypi dependency will add it to the native pyproject `project.dependencies` array
    /// or to the native `project.optional-dependencies` table if a feature is specified:
    /// - `pixi add --pypi boto3` will add `boto3` to the `project.dependencies` array
    /// - `pixi add --pypi boto3 --feature aws` will add `boto3` to the `project.dependencies.aws` array
    /// These dependencies will then be read by pixi as if they had been added to the pixi `pypi-dependencies` tables of the default or of a named feature.
    ///
    #[arg(required = true, verbatim_doc_comment)]
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

    /// Whether the pypi requirement should be editable
    #[arg(long, requires = "pypi")]
    pub editable: bool,
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
    project
        .manifest
        .add_platforms(spec_platforms.iter(), &FeatureName::Default)?;

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
                Some(args.editable),
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
    editable: Option<bool>,
) -> miette::Result<()> {
    for requirement in &requirements {
        // TODO: Get best version
        // Add the dependency to the project
        if platforms.is_empty() {
            project
                .manifest
                .add_pypi_dependency(requirement, None, feature_name, editable)?;
        } else {
            for platform in platforms.iter() {
                project.manifest.add_pypi_dependency(
                    requirement,
                    Some(*platform),
                    feature_name,
                    editable,
                )?;
            }
        }
    }
    let lock_file_usage = if no_update_lockfile {
        LockFileUsage::Frozen
    } else {
        LockFileUsage::Update
    };

    get_up_to_date_prefix(&project.default_environment(), lock_file_usage, no_install).await?;

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
                platform,
                grouped_environment.channels(),
                project.repodata_gateway(),
            )
            .await
            {
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
                updated_spec.version = determine_version_constraint(
                    &determine_latest_versions(project, specs_platforms, &name).await?,
                );
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
    get_up_to_date_prefix(&project.default_environment(), lock_file_usage, no_install).await?;

    project.save()?;

    Ok(())
}

/// Get all the latest versions found in the platforms repodata.
async fn determine_latest_versions(
    project: &Project,
    platforms: &[Platform],
    name: &PackageName,
) -> miette::Result<Vec<Version>> {
    // Get platforms to search for including NoArch
    let platforms = if platforms.is_empty() {
        let mut temp = project
            .default_environment()
            .platforms()
            .into_iter()
            .collect_vec();
        temp.push(Platform::NoArch);
        temp
    } else {
        let mut temp = platforms.to_vec();
        temp.push(Platform::NoArch);
        temp
    };

    // Get the records for the package
    let records = project
        .repodata_gateway()
        .query(
            project
                .default_environment()
                .channels()
                .into_iter()
                .cloned(),
            platforms,
            [name.clone()],
        )
        .recursive(false)
        .await
        .into_diagnostic()?;

    // Find the first non-empty channel
    let Some(priority_records) = records.into_iter().find(|records| !records.is_empty()) else {
        return Ok(vec![]);
    };

    // Find the maximum versions per platform
    let mut found_records: HashMap<String, Version> = HashMap::new();
    for record in priority_records.iter() {
        let version = record.package_record.version.version().clone();
        let platform = &record.package_record.subdir;
        found_records
            .entry(platform.clone())
            .and_modify(|max| {
                if &version > max {
                    *max = version.clone();
                }
            })
            .or_insert(version);
    }

    // Determine the version constraint based on the max of every channel and platform.
    Ok(found_records.into_values().collect())
}
/// Given several specs determines the highest installable version for them.
pub async fn determine_best_version<'p>(
    environment: &GroupedEnvironment<'p>,
    new_specs: &HashMap<PackageName, NamelessMatchSpec>,
    new_specs_type: SpecType,
    platform: Platform,
    channels: impl IntoIterator<Item = &'p Channel>,
    repodata_gateway: &Gateway,
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
    let fetch_repodata_start = Instant::now();
    let available_packages = repodata_gateway
        .query(
            channels.into_iter().cloned(),
            [platform, Platform::NoArch],
            dependencies.clone().into_match_specs(),
        )
        .recursive(true)
        .await
        .into_diagnostic()?;
    let total_records = available_packages.iter().map(RepoData::len).sum::<usize>();
    tracing::info!(
        "fetched {total_records} records in {:?}",
        fetch_repodata_start.elapsed()
    );

    // Construct a solver task to start solving.
    let task = rattler_solve::SolverTask {
        specs: dependencies
            .iter_specs()
            .map(|(name, spec)| MatchSpec::from_nameless(spec.clone(), Some(name.clone())))
            .collect(),
        available_packages: available_packages
            .iter()
            .map(RepoDataIter)
            .collect::<Vec<_>>(),
        virtual_packages: environment.virtual_packages(platform),
        locked_packages: vec![],
        pinned_packages: vec![],
        timeout: None,
        channel_priority: ChannelPriority::Strict,
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
