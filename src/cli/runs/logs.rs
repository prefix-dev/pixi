use clap::Parser;

use crate::{runs::DaemonRunsManager, Project};

/// Display the logs of a daemon task of the project. Print the stdout logs by default. Use `--stderr` to print the stderr logs.
/// Note that for now, the logs are not streamed. It means that if the task is still running, the logs will not be updated.
#[derive(Parser, Debug)]
pub struct Args {
    /// The name of the run to clear
    pub name: String,

    /// Print the stderr logs instead of the stdout logs
    #[clap(long)]
    pub err: bool,
}

pub async fn execute(project: Project, args: Args) -> miette::Result<()> {
    // Init the runs manager
    let runs_manager = DaemonRunsManager::new(&project);

    // Get the run
    let run = runs_manager.get_run(args.name)?;

    // Get the logs path
    let logs_path = if args.err {
        run.stderr_path()
    } else {
        run.stdout_path()
    };

    // Print the logs
    if logs_path.exists() {
        let logs = std::fs::read_to_string(logs_path).expect("Failed to read the logs file");

        match logs.as_str() {
            "" => miette::bail!("No logs found for the run '{}'", run.name),
            _ => println!("{}", logs),
        }
    } else {
        miette::bail!("No logs found for the run '{}'", run.name);
    }

    Ok(())
}
