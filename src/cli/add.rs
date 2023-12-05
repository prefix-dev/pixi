use crate::environment::{update_prefix, verify_prefix_location_unchanged};
use crate::prefix::Prefix;
use crate::project::SpecType;
use crate::{
    consts,
    lock_file::{load_lock_file, update_lock_file},
    project::python::PyPiRequirement,
    project::Project,
    virtual_packages::get_minimal_virtual_packages,
};
use clap::Parser;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::version_spec::{LogicalOperator, RangeOperator};
use rattler_conda_types::{
    MatchSpec, NamelessMatchSpec, PackageName, Platform, Version, VersionSpec,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{resolvo, SolverImpl};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;

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

    /// The path to 'pixi.toml'
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
}

impl SpecType {
    pub fn from_args(args: &Args) -> Self {
        if args.pypi {
            Self::Pypi
        } else if args.host {
            Self::Host
        } else if args.build {
            Self::Build
        } else {
            Self::Run
        }
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    let spec_type = SpecType::from_args(&args);
    let spec_platforms = &args.platform;

    // Sanity check of prefix location
    verify_prefix_location_unchanged(
        project
            .environment_dir()
            .join(consts::PREFIX_FILE_NAME)
            .as_path(),
    )?;

    // Add the platform if it is not already present
    let platforms_to_add = spec_platforms
        .iter()
        .filter(|p| !project.platforms().contains(p))
        .cloned()
        .collect::<Vec<Platform>>();
    project.add_platforms(platforms_to_add.iter())?;

    match spec_type {
        SpecType::Host | SpecType::Build | SpecType::Run => {
            let specs = args
                .specs
                .clone()
                .into_iter()
                .map(|s| MatchSpec::from_str(&s))
                .collect::<Result<Vec<_>, _>>()
                .into_diagnostic()?;
            add_conda_specs_to_project(
                &mut project,
                specs,
                spec_type,
                args.no_install,
                args.no_lockfile_update,
                spec_platforms,
            )
            .await
        }
        SpecType::Pypi => {
            // Parse specs as pep508_rs requirements
            let pep508_requirements = args
                .specs
                .clone()
                .into_iter()
                .map(|input| pep508_rs::Requirement::from_str(input.as_ref()).into_diagnostic())
                .collect::<miette::Result<Vec<_>>>()?;

            // Move those requirements into our custom PyPiRequirement
            let specs = pep508_requirements
                .into_iter()
                .map(|req| {
                    let name = rip::types::PackageName::from_str(req.name.as_str())?;
                    let requirement = PyPiRequirement::from(req);
                    Ok((name, requirement))
                })
                .collect::<Result<Vec<_>, rip::types::ParsePackageNameError>>()
                .into_diagnostic()?;

            add_pypi_specs_to_project(
                &mut project,
                specs,
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
    if !matches!(spec_type, SpecType::Run) {
        eprintln!(
            "Added these as {}.",
            console::style(spec_type.name()).bold()
        );
    }

    // Print something if we've added for platforms
    if !args.platform.is_empty() {
        eprintln!(
            "Added these only for platform(s): {}",
            console::style(args.platform.iter().join(", ")).bold()
        )
    }

    Ok(())
}

pub async fn add_pypi_specs_to_project(
    project: &mut Project,
    specs: Vec<(rip::types::PackageName, PyPiRequirement)>,
    specs_platforms: &Vec<Platform>,
    no_update_lockfile: bool,
    no_install: bool,
) -> miette::Result<()> {
    for (name, spec) in &specs {
        // TODO: Get best version
        // Add the dependency to the project
        if specs_platforms.is_empty() {
            project.add_pypi_dependency(name, spec)?;
        } else {
            for platform in specs_platforms.iter() {
                project.add_target_pypi_dependency(*platform, name.clone(), spec)?;
            }
        }
    }
    project.save()?;

    update_lockfile(project, None, no_install, no_update_lockfile).await?;

    Ok(())
}

pub async fn add_conda_specs_to_project(
    project: &mut Project,
    specs: Vec<MatchSpec>,
    spec_type: SpecType,
    no_install: bool,
    no_update_lockfile: bool,
    specs_platforms: &Vec<Platform>,
) -> miette::Result<()> {
    // Split the specs into package name and version specifier
    let new_specs = specs
        .into_iter()
        .map(|spec| match &spec.name {
            Some(name) => Ok((name.clone(), spec.into())),
            None => Err(miette::miette!("missing package name for spec '{spec}'")),
        })
        .collect::<miette::Result<HashMap<PackageName, NamelessMatchSpec>>>()?;

    // Get the current specs

    // Fetch the repodata for the project
    let sparse_repo_data = project.fetch_sparse_repodata().await?;

    // Determine the best version per platform
    let mut package_versions = HashMap::<PackageName, HashSet<Version>>::new();

    let platforms = if specs_platforms.is_empty() {
        project.platforms()
    } else {
        specs_platforms
    }
    .to_vec();
    for platform in platforms {
        let current_specs = match spec_type {
            SpecType::Host => project.host_dependencies(platform)?,
            SpecType::Build => project.build_dependencies(platform)?,
            SpecType::Run => project.dependencies(platform)?,
            _ => {
                return Err(miette::miette!(
                    "Cannot add any other spec types as conda packages"
                ))
            }
        };
        // Solve the environment with the new specs added
        let solved_versions = match determine_best_version(
            &new_specs,
            &current_specs,
            &sparse_repo_data,
            platform,
        ) {
            Ok(versions) => versions,
            Err(err) => {
                return Err(err).wrap_err_with(||miette::miette!(
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

    // Update the specs passed on the command line with the best available versions.
    for (name, spec) in new_specs {
        let versions_seen = package_versions
            .get(&name)
            .cloned()
            .expect("a version must have been previously selected");
        let updated_spec = if spec.version.is_none() {
            let mut updated_spec = spec.clone();
            updated_spec.version = determine_version_constraint(&versions_seen);
            updated_spec
        } else {
            spec
        };
        let spec = MatchSpec::from_nameless(updated_spec, Some(name));

        // Add the dependency to the project
        if specs_platforms.is_empty() {
            project.add_dependency(&spec, spec_type)?;
        } else {
            for platform in specs_platforms.iter() {
                project.add_target_dependency(*platform, &spec, spec_type)?;
            }
        }
    }
    project.save()?;

    update_lockfile(
        project,
        Some(sparse_repo_data),
        no_install,
        no_update_lockfile,
    )
    .await?;

    Ok(())
}

async fn update_lockfile(
    project: &Project,
    sparse_repo_data: Option<Vec<SparseRepoData>>,
    no_install: bool,
    no_update_lockfile: bool,
) -> miette::Result<()> {
    // Update the lock file
    let lock_file = if !no_update_lockfile {
        Some(update_lock_file(project, load_lock_file(project).await?, sparse_repo_data).await?)
    } else {
        None
    };

    if let Some(lock_file) = lock_file {
        if !no_install {
            let platform = Platform::current();
            if project.platforms().contains(&platform) {
                // Get the currently installed packages
                let prefix = Prefix::new(project.root().join(".pixi/env"))?;
                let installed_packages = prefix.find_installed_packages(None).await?;

                // Update the prefix
                update_prefix(
                    project.pypi_package_db()?,
                    &prefix,
                    installed_packages,
                    &lock_file,
                    platform,
                )
                .await?;
            } else {
                eprintln!("{} skipping installation of environment because your platform ({platform}) is not supported by this project.", console::style("!").yellow().bold())
            }
        }
    }
    Ok(())
}
/// Given several specs determines the highest installable version for them.
pub fn determine_best_version(
    new_specs: &HashMap<PackageName, NamelessMatchSpec>,
    current_specs: &IndexMap<PackageName, NamelessMatchSpec>,
    sparse_repo_data: &[SparseRepoData],
    platform: Platform,
) -> miette::Result<HashMap<PackageName, Version>> {
    let combined_specs = current_specs
        .iter()
        .chain(new_specs.iter())
        .map(|(name, spec)| (name.clone(), spec.clone()))
        .collect::<HashMap<_, _>>();

    // Extract the package names from all the dependencies
    let package_names = combined_specs.keys().cloned().collect_vec();

    // Get the repodata for the current platform and for NoArch
    let platform_sparse_repo_data = sparse_repo_data.iter().filter(|sparse| {
        sparse.subdir() == platform.as_str() || sparse.subdir() == Platform::NoArch.as_str()
    });

    // Load only records we need for this platform
    let available_packages = SparseRepoData::load_records_recursive(
        platform_sparse_repo_data,
        package_names.iter().cloned(),
        None,
    )
    .into_diagnostic()?;

    // Construct a solver task to start solving.
    let task = rattler_solve::SolverTask {
        specs: combined_specs
            .iter()
            .map(|(name, spec)| MatchSpec::from_nameless(spec.clone(), Some(name.clone())))
            .collect(),

        available_packages: &available_packages,

        virtual_packages: get_minimal_virtual_packages(platform)
            .into_iter()
            .map(Into::into)
            .collect(),

        // TODO: Add the information from the current lock file here.
        locked_packages: vec![],

        pinned_packages: vec![],
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
        .bump();
    Some(VersionSpec::Group(
        LogicalOperator::And,
        vec![
            VersionSpec::Range(RangeOperator::GreaterEquals, lower_bound),
            VersionSpec::Range(RangeOperator::Less, upper_bound),
        ],
    ))
}

#[cfg(test)]
mod test {
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
