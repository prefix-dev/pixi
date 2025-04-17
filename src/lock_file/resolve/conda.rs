use ahash::HashMap;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_manifest::ChannelPriority;
use pixi_record::{PixiRecord, SourceRecord};
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, RepoDataRecord};
use rattler_repodata_gateway::RepoData;
use rattler_solve::{resolvo, SolveStrategy, SolverImpl};
use url::Url;

use crate::{
    build::{SourceCheckout, SourceMetadata},
    lock_file::LockedCondaPackages,
};

/// Solves the conda package environment for the given input. This function is
/// async because it spawns a background task for the solver. Since solving is a
/// CPU intensive task we do not want to block the main task.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_conda(
    specs: Vec<MatchSpec>,
    virtual_packages: Vec<GenericVirtualPackage>,
    locked_packages: Vec<RepoDataRecord>,
    available_repodata: Vec<RepoData>,
    available_source_packages: Vec<SourceMetadata>,
    channel_priority: ChannelPriority,
    exclude_newer: Option<chrono::DateTime<chrono::Utc>>,
    solve_strategy: SolveStrategy,
) -> miette::Result<LockedCondaPackages> {
    tokio::task::spawn_blocking(move || {
        // Combine the repodata from the source packages and from registry channels.
        let mut url_to_source_package = HashMap::default();
        for source_metadata in available_source_packages {
            for record in source_metadata.records {
                let url = unique_url(&source_metadata.source, &record);
                let repodata_record = RepoDataRecord {
                    package_record: record.package_record.clone(),
                    url: url.clone(),
                    file_name: format!(
                        "{}-{}-{}.source",
                        record.package_record.name.as_normalized(),
                        &record.package_record.version,
                        &record.package_record.build
                    ),
                    channel: None,
                };
                url_to_source_package.insert(url, (record, repodata_record));
            }
        }

        let mut solvable_records = Vec::with_capacity(available_repodata.len() + 1);
        solvable_records.push(
            url_to_source_package
                .values()
                .map(|(_, record)| record)
                .collect_vec(),
        );
        for repo_data in &available_repodata {
            solvable_records.push(repo_data.iter().collect_vec());
        }

        // Construct a solver task that we can start solving.
        let task = rattler_solve::SolverTask {
            specs,
            locked_packages,
            virtual_packages,
            channel_priority: channel_priority.into(),
            exclude_newer,
            strategy: solve_strategy,
            ..rattler_solve::SolverTask::from_iter(solvable_records)
        };

        // Solve the task
        let solved = resolvo::Solver.solve(task).into_diagnostic()?;

        Ok(solved
            .records
            .into_iter()
            .map(|record| {
                url_to_source_package.remove(&record.url).map_or_else(
                    || PixiRecord::Binary(record),
                    |(source_record, _repodata_record)| PixiRecord::Source(source_record),
                )
            })
            .collect_vec())
    })
    .await
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(e) => std::panic::resume_unwind(e),
        Err(_err) => Err(miette::miette!("cancelled")),
    })
}

fn unique_url(checkout: &SourceCheckout, source: &SourceRecord) -> Url {
    let mut url = Url::from_directory_path(&checkout.path)
        .expect("expected source checkout to be a valid url");

    // Add unique identifiers to the URL.
    url.query_pairs_mut()
        .append_pair("name", source.package_record.name.as_source())
        .append_pair("version", &source.package_record.version.as_str())
        .append_pair("build", &source.package_record.build)
        .append_pair("subdir", &source.package_record.subdir);

    url
}
