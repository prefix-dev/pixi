use clap::Parser;

use crate::{
    runs::{DaemonRun, DaemonRunsManager, SystemInfo},
    Project,
};

/// Kill all the detached run that are not terminated. It will send a SIGTERM signals to the processes.
#[derive(Parser, Debug)]
pub struct Args {
    /// Whether to also clear the run from the history
    #[clap(short, long)]
    pub clear: bool,
}

pub async fn execute(project: Project, args: Args) -> miette::Result<()> {
    // Init the runs manager
    let runs_manager = DaemonRunsManager::new(&project);

    // Refresh the system info about processes and PIDs
    SystemInfo::refresh();

    // get all the non alive runs
    let all_runs = runs_manager.runs();
    let runs: Vec<&DaemonRun> = all_runs.iter().filter(|run| run.is_alive()).collect();

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

        let message_actions = match args.clear {
            true => {
                run.clear_force()?;
                "killed and cleared"
            }
            false => "killed",
        };

        // Emit success
        eprintln!(
            "{}Run called '{}' correctly {}.",
            console::style(console::Emoji("✔ ", "")).green(),
            run.name,
            message_actions
        );
    }

    // Emit success
    eprintln!(
        "{}All the runs correctly {}",
        console::style(console::Emoji("✔ ", "")).green(),
        match args.clear {
            true => "killed and cleared",
            false => "killed",
        }
    );

    Ok(())
}
