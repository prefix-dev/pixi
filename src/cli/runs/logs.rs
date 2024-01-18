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

    // Print the logs
    let logs = match args.err {
        true => run.read_stderr()?,
        false => run.read_stdout()?,
    };

    match logs.as_str() {
        "" => miette::bail!("No logs found for the run '{}'", run.name),
        _ => println!("{}", logs),
    }

    Ok(())
}
