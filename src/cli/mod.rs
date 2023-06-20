use super::util::IndicatifWriter;
use clap::{CommandFactory, Parser};
use clap_complete::Shell;
use clap_verbosity_flag::Verbosity;

use crate::Project;
use crate::{environment::get_up_to_date_prefix, progress};
use anyhow::Error;
use tracing_subscriber::{filter::LevelFilter, util::SubscriberInitExt, EnvFilter};

mod add;
mod global;
mod init;
mod run;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// The verbosity level
    /// (-v for verbose, -vv for debug, -vvv for trace, -q for quiet)
    #[command(flatten)]
    verbose: Verbosity,
}

/// Generates a completion script for a shell.
#[derive(Parser, Debug)]
pub struct CompletionCommand {
    /// The shell to generate a completion script for (defaults to 'bash').
    #[arg(short, long)]
    shell: Option<Shell>,
}

#[derive(Parser, Debug)]
enum Command {
    Completion(CompletionCommand),
    Init(init::Args),
    #[clap(alias = "a")]
    Add(add::Args),
    #[clap(alias = "r")]
    Run(run::Args),
    #[clap(alias = "g")]
    Global(global::Args),
}

fn completion(args: CompletionCommand) -> Result<(), Error> {
    clap_complete::generate(
        args.shell.or(Shell::from_env()).unwrap_or(Shell::Bash),
        &mut Args::command(),
        "pixi",
        &mut std::io::stdout(),
    );

    Ok(())
}

/// Run the project initialization when there is a manifest available.
/// This is run when only running `pixi`, which aligns with yarns implementation.
async fn default() -> Result<(), Error> {
    let project = Project::discover()?;
    get_up_to_date_prefix(&project).await?;
    // Emit success
    eprintln!(
        "{}Project in {} is ready to use!",
        console::style(console::Emoji("✔ ", "")).green(),
        project.root().display()
    );
    Ok(())
}

pub async fn execute() -> anyhow::Result<()> {
    let args = Args::parse();

    let level_filter = match args.verbose.log_level_filter() {
        clap_verbosity_flag::LevelFilter::Off => LevelFilter::OFF,
        clap_verbosity_flag::LevelFilter::Error => LevelFilter::ERROR,
        clap_verbosity_flag::LevelFilter::Warn => LevelFilter::WARN,
        clap_verbosity_flag::LevelFilter::Info => LevelFilter::INFO,
        clap_verbosity_flag::LevelFilter::Debug => LevelFilter::DEBUG,
        clap_verbosity_flag::LevelFilter::Trace => LevelFilter::TRACE,
    };

    let env_filter = EnvFilter::builder()
        .with_default_directive(level_filter.into())
        .from_env()?
        // filter logs from apple codesign because they are very noisy
        .add_directive("apple_codesign=off".parse()?);

    // Setup the tracing subscriber
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(IndicatifWriter::new(progress::global_multi_progress()))
        .without_time()
        .finish()
        .try_init()?;

    match args.command {
        Some(Command::Completion(cmd)) => completion(cmd),
        Some(Command::Init(cmd)) => init::execute(cmd).await,
        Some(Command::Add(cmd)) => add::execute(cmd).await,
        Some(Command::Run(cmd)) => run::execute(cmd).await,
        Some(Command::Global(cmd)) => global::execute(cmd).await,
        None => default().await,
    }
}
