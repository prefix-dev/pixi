use clap::Parser;

use crate::{
    runs::{DaemonRun, DaemonRunsManager},
    Project,
};

/// Kill all the detached run that are not terminated. It will send a SIGTERM signals to the processes.
#[derive(Parser, Debug)]
pub struct Args {}

pub async fn execute(project: Project, _args: Args) -> miette::Result<()> {
    // Init the runs manager
    let runs_manager = DaemonRunsManager::new(&project);

    // get all the non running runs
    let all_runs = runs_manager.runs();
    let runs: Vec<&DaemonRun> = all_runs.iter().filter(|run| run.is_running()).collect();

    if runs.is_empty() {
        eprintln!(
            "{}No running runs to kill",
            console::style(console::Emoji("✔ ", "")).green(),
        );
        return Ok(());
    }

    // Clear all the runs
    for run in runs {
        run.kill()?;

        // Emit success
        eprintln!(
            "{}Run called '{}' correctly killed",
            console::style(console::Emoji("✔ ", "")).green(),
            run.name
        );
    }

    // Emit success
    eprintln!(
        "{}All the running runs correctly killed",
        console::style(console::Emoji("✔ ", "")).green(),
    );

    Ok(())
}
