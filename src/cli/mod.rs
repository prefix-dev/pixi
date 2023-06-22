use super::util::IndicatifWriter;
use clap::{CommandFactory, Parser};
use clap_complete::Shell;
use clap_verbosity_flag::Verbosity;

use crate::progress;
use anyhow::Error;
use tracing_subscriber::{filter::LevelFilter, util::SubscriberInitExt, EnvFilter};

mod add;
mod global;
mod init;
mod install;
mod run;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
#[clap(arg_required_else_help = true)]
struct Args {
    #[command(subcommand)]
    command: Command,

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
    Install(install::Args),
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
        Command::Completion(cmd) => completion(cmd),
        Command::Init(cmd) => init::execute(cmd).await,
        Command::Add(cmd) => add::execute(cmd).await,
        Command::Run(cmd) => run::execute(cmd).await,
        Command::Global(cmd) => global::execute(cmd).await,
        Command::Install(cmd) => install::execute(cmd).await,
    }
}
