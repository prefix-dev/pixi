use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, RepoDataRecord};
use rattler_repodata_gateway::RepoData;
use rattler_solve::{resolvo, ChannelPriority, SolverImpl};

use crate::lock_file::LockedCondaPackages;

/// Solves the conda package environment for the given input. This function is
/// async because it spawns a background task for the solver. Since solving is a
/// CPU intensive task we do not want to block the main task.
pub async fn resolve_conda(
    specs: Vec<MatchSpec>,
    virtual_packages: Vec<GenericVirtualPackage>,
    locked_packages: Vec<RepoDataRecord>,
    available_packages: Vec<RepoData>,
    source_packages: Vec<Vec<RepoDataRecord>>,
    channel_priority: ChannelPriority,
) -> miette::Result<LockedCondaPackages> {
    tokio::task::spawn_blocking(move || {
        let mut all_packages = Vec::with_capacity(available_packages.len() + 1);
        for repo_data in &source_packages {
            all_packages.push(repo_data.iter().collect_vec());
        }
        for repo_data in &available_packages {
            all_packages.push(repo_data.iter().collect_vec());
        }

        // Construct a solver task that we can start solving.
        let task = rattler_solve::SolverTask {
            specs,
            locked_packages,
            virtual_packages,
            channel_priority,
            ..rattler_solve::SolverTask::from_iter(all_packages)
        };

        // Solve the task
        resolvo::Solver.solve(task).into_diagnostic()
    })
    .await
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(e) => std::panic::resume_unwind(e),
        Err(_err) => Err(miette::miette!("cancelled")),
    })
}
