use clap::Parser;

use crate::{
    runs::{DaemonRun, DaemonRunsManager},
    Project,
};

/// Clear all the terminated detached runs. It will remove the pid, the logs and the infos files from the runs directory.
#[derive(Parser, Debug)]
pub struct Args {}

pub async fn execute(project: Project, _args: Args) -> miette::Result<()> {
    // Init the runs manager
    let runs_manager = DaemonRunsManager::new(&project);

    // get all the non running runs
    let all_runs = runs_manager.runs();
    let runs: Vec<&DaemonRun> = all_runs.iter().filter(|run| !run.is_running()).collect();

    if runs.len() == 0 {
        eprintln!(
            "{}No terminated runs to clear",
            console::style(console::Emoji("✔ ", "")).green(),
        );
        return Ok(());
    }

    // Clear all the runs
    for run in runs {
        run.clear()?;

        // Emit success
        eprintln!(
            "{}Run called '{}' correctly cleared",
            console::style(console::Emoji("✔ ", "")).green(),
            run.name
        );
    }

    // Emit success
    eprintln!(
        "{}All the terminated runs correctly cleared",
        console::style(console::Emoji("✔ ", "")).green(),
    );

    Ok(())
}
