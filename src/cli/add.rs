use crate::{
    environment::{load_lock_file, update_lock_file},
    project::Project,
    virtual_packages::get_minimal_virtual_packages,
};
use anyhow::Context;
use clap::Parser;
use itertools::Itertools;
use rattler_conda_types::{
    version_spec::VersionOperator, MatchSpec, NamelessMatchSpec, Platform, Version, VersionSpec,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{LibsolvRepoData, SolverBackend};
use std::collections::HashMap;

/// Adds a dependency to the project
#[derive(Parser, Debug)]
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
    specs: Vec<MatchSpec>,
}

pub async fn execute(args: Args) -> anyhow::Result<()> {
    // Determine the location and metadata of the current project
    let mut project = Project::discover()?;

    // Split the specs into package name and version specifier
    let new_specs = args
        .specs
        .into_iter()
        .map(|spec| match &spec.name {
            Some(name) => Ok((name.clone(), spec.into())),
            None => Err(anyhow::anyhow!("missing package name for spec '{spec}'")),
        })
        .collect::<anyhow::Result<HashMap<String, NamelessMatchSpec>>>()?;

    // Get the current specs
    let current_specs = project.dependencies()?;

    // Fetch the repodata for the project
    let sparse_repo_data = project.fetch_sparse_repodata().await?;

    // Determine the best version per platform
    let mut best_versions = HashMap::new();
    for platform in project.platforms()? {
        // Solve the environment with the new specs added
        let solved_versions = match determine_best_version(
            &new_specs,
            &current_specs,
            &sparse_repo_data,
            platform,
        ) {
            Ok(versions) => versions,
            Err(err) => {
                return Err(err).context(anyhow::anyhow!(
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
        project.add_dependency(&spec)?;
        added_specs.push(spec);
    }

    // Update the lock file and write to disk
    update_lock_file(
        &project,
        load_lock_file(&project).await?,
        Some(sparse_repo_data),
    )
    .await?;
    project.save()?;

    for spec in added_specs {
        eprintln!(
            "{}Added {}",
            console::style(console::Emoji("✔ ", "")).green(),
            spec
        );
    }

    Ok(())
}

/// Given several specs determines the highest installable version for them.
pub fn determine_best_version(
    new_specs: &HashMap<String, NamelessMatchSpec>,
    current_specs: &HashMap<String, NamelessMatchSpec>,
    sparse_repo_data: &[SparseRepoData],
    platform: Platform,
) -> anyhow::Result<HashMap<String, Version>> {
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
    )?;

    // Construct a solver task to start solving.
    let task = rattler_solve::SolverTask {
        specs: combined_specs
            .iter()
            .map(|(name, spec)| MatchSpec::from_nameless(spec.clone(), Some(name.clone())))
            .collect(),

        available_packages: available_packages
            .iter()
            .map(|records| LibsolvRepoData::from_records(records)),

        virtual_packages: get_minimal_virtual_packages(platform)
            .into_iter()
            .map(Into::into)
            .collect(),

        // TODO: Add the information from the current lock file here.
        locked_packages: vec![],

        pinned_packages: vec![],
    };

    let records = rattler_solve::LibsolvBackend.solve(task)?;

    // Determine the versions of the new packages
    Ok(records
        .into_iter()
        .filter(|record| new_specs.contains_key(&record.package_record.name))
        .map(|record| (record.package_record.name, record.package_record.version))
        .collect())
}
