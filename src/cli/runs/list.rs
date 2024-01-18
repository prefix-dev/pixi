use clap::Parser;
use comfy_table::{presets::NOTHING, Attribute, Cell, ContentArrangement, Table};

use crate::{
    runs::{DaemonRunState, DaemonRunsManager, SystemInfo},
    Project,
};

/// List all the daemon tasks of the project.
#[derive(Parser, Debug)]
pub struct Args {
    /// Whether to output in json format
    #[arg(long)]
    pub json: bool,

    /// Whether to output in pretty json format
    #[arg(long)]
    pub json_pretty: bool,
}

pub async fn execute(project: Project, args: Args) -> miette::Result<()> {
    // Init the runs manager
    let runs_manager = DaemonRunsManager::new(&project);

    // Refresh the system info about processes and PIDs
    SystemInfo::refresh();

    // Get all the run states
    let run_states: Result<Vec<DaemonRunState>, _> = runs_manager
        .runs()
        .into_iter()
        .map(|run| run.state())
        .collect();

    match run_states {
        Ok(mut run_states) => {
            // Print the runs
            if run_states.is_empty() {
                eprintln!(
                    "{}No runs found",
                    console::style(console::Emoji("✔ ", "")).green(),
                );
            } else {
                // Sort by start date by default
                run_states.sort_by(|a, b| b.start_date.cmp(&a.start_date));

                if args.json || args.json_pretty {
                    print_as_json(&run_states, args.json_pretty);
                } else {
                    print_as_table(&run_states);
                }
            }
        }
        Err(err) => {
            miette::bail!("Failed to get runs: {}", err);
        }
    }

    Ok(())
}

fn print_as_table(run_states: &Vec<DaemonRunState>) {
    // Initialize table
    let mut table = Table::new();

    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic);

    // Add headers
    table.set_header(vec![
        Cell::new("Name").add_attribute(Attribute::Bold),
        Cell::new("Status").add_attribute(Attribute::Bold),
        Cell::new("PID").add_attribute(Attribute::Bold),
        Cell::new("Start Date").add_attribute(Attribute::Bold),
        Cell::new("Task").add_attribute(Attribute::Bold),
        Cell::new("Stdout Size").add_attribute(Attribute::Bold),
        Cell::new("Stderr Size").add_attribute(Attribute::Bold),
    ]);

    for state in run_states {
        table.add_row(vec![
            Cell::new(&state.name),
            Cell::new(&state.status),
            Cell::new(state.pid),
            Cell::new(&state.start_date.format("%Y-%m-%d %H:%M:%S")),
            Cell::new(&state.task.join(" ")),
            Cell::new(state.stdout_length),
            Cell::new(state.stderr_length),
        ]);
    }

    println!("{table}");
}

fn print_as_json(run_states: &Vec<DaemonRunState>, json_pretty: bool) {
    let json_string = if json_pretty {
        serde_json::to_string_pretty(&run_states)
    } else {
        serde_json::to_string(&run_states)
    }
    .expect("Cannot serialize to JSON");

    println!("{}", json_string);
}
