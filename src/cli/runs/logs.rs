use std::{
    fs::File,
    io::{self, BufRead, Read, Seek},
};

use clap::Parser;
use miette::IntoDiagnostic;
use rev_buf_reader::RevBufReader;

use crate::{runs::DaemonRunsManager, Project};

/// Print the logs of a detached runs of the project. It prints the stdout logs
/// by default. Use `--stderr` to print the stderr logs.
#[derive(Parser, Debug)]
pub struct Args {
    /// The name of the run to clear
    pub name: String,

    /// Print the stderr logs instead of the stdout logs
    #[clap(long)]
    pub stderr: bool,

    /// The number of lines to print starting from the end of the logs. If 0, print the whole logs.
    #[clap(short, long, default_value = "0")]
    pub lines: usize,

    /// Whether to follow the logs or not. If true, the logs will be streamed.
    #[clap(short, long)]
    pub follow: bool,
}

pub async fn execute(project: Project, args: Args) -> miette::Result<()> {
    // Init the runs manager
    let runs_manager = DaemonRunsManager::new(&project);

    // Get the run
    let run = runs_manager.get_run(args.name)?;

    // Get logs path
    let logs_path = match args.stderr {
        true => run.stderr_path(),
        false => run.stdout_path(),
    };

    if !logs_path.exists() {
        miette::bail!("No logs found for the run '{}'", run.name);
    }

    // Read the last n lines of logs_path without reading the whole file
    let mut file = File::open(logs_path).into_diagnostic()?;
    let lines = lines_from_file(&mut file, args.lines)?;

    // Print the logs
    println!("{}", lines.join("\n"));

    // Follow the logs eventually
    if args.follow {
        // Seek to the end of the file
        file.seek(io::SeekFrom::End(0)).into_diagnostic()?;

        // Create a buffer to store the logs
        let mut buffer = String::new();

        // Read the logs
        loop {
            // Read the logs
            file.read_to_string(&mut buffer).into_diagnostic()?;

            // Print the logs
            print!("{}", buffer);

            // Clear the buffer
            buffer.clear();

            // Wait for the next logs
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    Ok(())
}

fn lines_from_file(file: &mut File, limit: usize) -> miette::Result<Vec<String>> {
    let buf = RevBufReader::new(file);
    let all_lines = buf.lines();

    let output = match limit {
        0 => all_lines.collect::<Vec<Result<String, std::io::Error>>>(),
        _ => all_lines
            .take(limit)
            .collect::<Vec<Result<String, std::io::Error>>>(),
    }
    .into_iter()
    .filter_map(|l| l.ok())
    .rev()
    .collect::<Vec<String>>();

    Ok(output)
}
