use clap::Parser;

use crate::{runs::DaemonRunsManager, Project};

/// Kill a detached run. It will send a SIGTERM signal to the process.
#[derive(Parser, Debug)]
pub struct Args {
    /// The name of the run to kill
    pub name: String,
}

pub async fn execute(project: Project, args: Args) -> miette::Result<()> {
    // Init the runs manager
    let runs_manager = DaemonRunsManager::new(&project);

    // Get the run
    let run = runs_manager.get_run(args.name)?;

    // Kill the run
    run.kill()?;

    // Emit success
    eprintln!(
        "{}Run called '{}' correctly killed",
        console::style(console::Emoji("✔ ", "")).green(),
        run.name
    );

    Ok(())
}
