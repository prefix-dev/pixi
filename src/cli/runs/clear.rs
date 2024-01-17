use clap::Parser;

use crate::{runs::DaemonRunsManager, Project};

/// Clear a detached run. It only works on terminated runs. It will remove the pid, the logs and the infos files from the runs directory.
#[derive(Parser, Debug)]
pub struct Args {
    /// The name of the run to clear
    pub name: String,
}

pub async fn execute(project: Project, args: Args) -> miette::Result<()> {
    // Init the runs manager
    let runs_manager = DaemonRunsManager::new(&project);

    // Get the run
    let run = runs_manager.get_run(args.name)?;

    // Clear the run
    run.clear()?;

    // Emit success
    eprintln!(
        "{}Run called '{}' correctly cleared",
        console::style(console::Emoji("✔ ", "")).green(),
        run.name
    );

    Ok(())
}
