use clap::Parser;

use crate::{runs::DaemonRunsManager, Project};

/// Kill a detached run. It will send a SIGTERM signal to the process.
#[derive(Parser, Debug)]
pub struct Args {
    /// The name of the run to kill
    pub name: String,

    /// Whether to also clear the run from the history
    #[clap(short, long)]
    pub clear: bool,
}

pub async fn execute(project: Project, args: Args) -> miette::Result<()> {
    // Init the runs manager
    let runs_manager = DaemonRunsManager::new(&project);

    // Get the run
    let run = runs_manager.get_run(args.name)?;

    // let system = SystemInfo::get();

    // let proc = system.process(run.read_pid().unwrap()).unwrap();

    // // find all the child processes of a given pid
    // let children = system
    //     .processes()
    //     .into_iter()
    //     .filter(|(_pid, process)| process.parent() == Some(proc.pid()))
    //     .collect::<Vec<_>>();

    // println!("children: {:?}", children.iter().map(|(pid, _)| pid).collect::<Vec<_>>());

    // Kill the run
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

    Ok(())
}
