use crate::project::Project;
use clap::Parser;
use futures::StreamExt;
use rattler_conda_types::{version_spec::VersionOperator, MatchSpec, Version, VersionSpec};
use std::collections::HashMap;

/// Adds a dependency to the project
#[derive(Parser, Debug)]
pub struct Args {
    specs: Vec<MatchSpec>,
}

pub async fn execute(mut args: Args) -> anyhow::Result<()> {
    // Determine the location and metadata of the current project
    let mut project = Project::discover()?;

    // Check if there are specs that do not specify an explicit version
    let has_unspecified_version = args.specs.iter().any(|spec| spec.version.is_none());
    let sparse_repo_data = if has_unspecified_version {
        project.fetch_sparse_repodata().await?
    } else {
        Default::default()
    };

    // Add all the specs to the project
    for spec in args.specs.iter_mut() {
        // Get the name of the package to add
        let name = spec
            .name
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("glob packages are not supported"))?;

        // If no version is specified, determine the latest version and use that version
        if spec.version.is_none() {
            // Find the version that is supported by all non-noarch platforms.
            let all_package_records = sparse_repo_data
                .iter()
                .map(|repodata| repodata.load_records(name))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten();

            let max_versions =
                all_package_records.fold(HashMap::<String, Version>::new(), |mut init, record| {
                    init.entry(record.package_record.subdir.clone())
                        .and_modify(|version| {
                            if &*version < &record.package_record.version {
                                *version = record.package_record.version.clone()
                            }
                        })
                        .or_insert(record.package_record.version.clone());
                    init
                });

            spec.version = max_versions
                .into_values()
                .min()
                .map(|version| VersionSpec::Operator(VersionOperator::StartsWith, version))
        }

        project.add_dependency(spec)?;
    }

    // Save the project to disk
    project.save()?;

    for spec in args.specs {
        eprintln!(
            "{}Added {}",
            console::style(console::Emoji("✔ ", "")).green(),
            spec
        );
    }

    Ok(())
}
