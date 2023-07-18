use crate::environment::update_prefix;
use crate::prefix::Prefix;
use crate::project::SpecType;
use crate::{
    environment::{load_lock_file, update_lock_file},
    project::Project,
    virtual_packages::get_minimal_virtual_packages,
};
use clap::Parser;
use console::style;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::{
    version_spec::VersionOperator, MatchSpec, NamelessMatchSpec, Platform, Version, VersionSpec,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{libsolv_rs, SolverImpl};
use std::collections::HashMap;
use std::path::PathBuf;

/// Adds a dependency to the project
#[derive(Parser, Debug, Default)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specify the dependencies you wish to add to the project.
    ///
    /// All dependencies should be defined as MatchSpec. If no specific version is
    /// provided, the latest version compatible with your project will be chosen automatically.
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
    /// - `pixi add python --platform linux-64 --platform osx-arm64`: Will add the latest version of python for linux-64 and osx-arm64 platforms.
    ///
    /// - `pixi add python --build`: Will add the latest version of python for as a build dependency.
    ///
    /// Mixing `--platform` and `--build`/`--host` is supported
    ///
    pub specs: Vec<MatchSpec>,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// This is a host dependency
    #[arg(long, conflicts_with = "build")]
    pub host: bool,

    /// This is a build dependency
    #[arg(long, conflicts_with = "host")]
    pub build: bool,

    /// Don't install the package to the environment, only add the package to the lock-file.
    #[arg(long)]
    pub no_install: bool,

    /// The platform(s) for which the dependency should be added
    #[arg(long, short)]
    pub platform: Vec<Platform>,
}

impl SpecType {
    pub fn from_args(args: &Args) -> Self {
        if args.host {
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
    let spec_platforms = args.platform;

    // Add the platform if it is not already present
    let platforms_to_add = spec_platforms
        .iter()
        .filter(|p| !project.platforms().contains(p))
        .cloned()
        .collect::<Vec<Platform>>();
    project.add_platforms(platforms_to_add.iter())?;

    add_specs_to_project(
        &mut project,
        args.specs,
        spec_type,
        args.no_install,
        spec_platforms,
    )
    .await
}

pub async fn add_specs_to_project(
    project: &mut Project,
    specs: Vec<MatchSpec>,
    spec_type: SpecType,
    no_install: bool,
    specs_platforms: Vec<Platform>,
) -> miette::Result<()> {
    // Split the specs into package name and version specifier
    let new_specs = specs
        .into_iter()
        .map(|spec| match &spec.name {
            Some(name) => Ok((name.clone(), spec.into())),
            None => Err(miette::miette!("missing package name for spec '{spec}'")),
        })
        .collect::<miette::Result<HashMap<String, NamelessMatchSpec>>>()?;

    // Get the current specs

    // Fetch the repodata for the project
    let sparse_repo_data = project.fetch_sparse_repodata().await?;

    // Determine the best version per platform
    let mut best_versions = HashMap::new();

    let platforms = specs_platforms.clone();
    for platform in platforms {
        let current_specs = match spec_type {
            SpecType::Host => project.host_dependencies(platform)?,
            SpecType::Build => project.build_dependencies(platform)?,
            SpecType::Run => project.dependencies(platform)?,
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
                        new_specs.keys().join(", ")
                    ));
            }
        };

        // Determine the minimum compatible constraining version.
        for (name, version) in solved_versions {
            match best_versions.get_mut(&name) {
                Some(prev) => {
                    if *prev > version {
                        *prev = version
                    }
                }
                None => {
                    best_versions.insert(name, version);
                }
            }
        }
    }

    // Update the specs passed on the command line with the best available versions.
    let mut added_specs = Vec::new();
    for (name, spec) in new_specs {
        let best_version = best_versions
            .get(&name)
            .cloned()
            .expect("a version must have been previously selected");
        let updated_spec = if spec.version.is_none() {
            let mut updated_spec = spec.clone();
            updated_spec.version = Some(VersionSpec::Operator(
                VersionOperator::StartsWith,
                best_version,
            ));
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

        added_specs.push(spec);
    }

    // Update the lock file and write to disk
    let lock_file = update_lock_file(
        project,
        load_lock_file(project).await?,
        Some(sparse_repo_data),
    )
    .await?;
    project.save()?;

    if !no_install {
        let platform = Platform::current();
        if project.platforms().contains(&platform) {
            // Get the currently installed packages
            let prefix = Prefix::new(project.root().join(".pixi/env"))?;
            let installed_packages = prefix.find_installed_packages(None).await?;

            // Update the prefix
            update_prefix(&prefix, installed_packages, &lock_file, platform).await?;
        } else {
            eprintln!("{} skipping installation of environment because your platform ({platform}) is not supported by this project.", style("!").yellow().bold())
        }
    }

    for spec in added_specs {
        eprintln!(
            "{}Added {}",
            console::style(console::Emoji("✔ ", "")).green(),
            spec,
        );
    }

    // Print if it is something different from host and dep
    match spec_type {
        SpecType::Host => eprintln!("Added these as host dependencies."),
        SpecType::Build => eprintln!("Added these as build dependencies."),
        SpecType::Run => {}
    };

    // Print something if we've added for platforms
    if !specs_platforms.is_empty() {
        eprintln!(
            "Added these only for platform(s): {}",
            specs_platforms.iter().join(", ")
        )
    }

    Ok(())
}

/// Given several specs determines the highest installable version for them.
pub fn determine_best_version(
    new_specs: &HashMap<String, NamelessMatchSpec>,
    current_specs: &IndexMap<String, NamelessMatchSpec>,
    sparse_repo_data: &[SparseRepoData],
    platform: Platform,
) -> miette::Result<HashMap<String, Version>> {
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

    let records = libsolv_rs::Solver.solve(task).into_diagnostic()?;

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
