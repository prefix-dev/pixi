use miette::IntoDiagnostic;
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, RepoDataRecord};
use rattler_repodata_gateway::RepoData;
use rattler_solve::{resolvo, ChannelPriority, RepoDataIter, SolverImpl};

use crate::lock_file::LockedCondaPackages;

/// Solves the conda package environment for the given input. This function is async because it
/// spawns a background task for the solver. Since solving is a CPU intensive task we do not want to
/// block the main task.
pub async fn resolve_conda(
    specs: Vec<MatchSpec>,
    virtual_packages: Vec<GenericVirtualPackage>,
    locked_packages: Vec<RepoDataRecord>,
    available_packages: Vec<RepoData>,
) -> miette::Result<LockedCondaPackages> {
    tokio::task::spawn_blocking(move || {
        // Construct a solver task that we can start solving.
        let task = rattler_solve::SolverTask {
            specs,
            available_packages: available_packages
                .iter()
                .map(RepoDataIter)
                .collect::<Vec<_>>(),
            locked_packages,
            pinned_packages: vec![],
            virtual_packages,
            timeout: None,
            channel_priority: ChannelPriority::Strict,
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
