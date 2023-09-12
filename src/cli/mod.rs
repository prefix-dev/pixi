use super::util::IndicatifWriter;
use crate::progress;
use clap::{CommandFactory, Parser};
use clap_complete;
use clap_verbosity_flag::Verbosity;
use miette::IntoDiagnostic;
use rattler_shell::shell::{Shell, ShellEnum};
use std::io::{IsTerminal, Write};
use std::str::FromStr;
use tracing_subscriber::{filter::LevelFilter, util::SubscriberInitExt, EnvFilter};

pub mod add;
pub mod auth;
pub mod global;
pub mod info;
pub mod init;
pub mod install;
pub mod run;
pub mod search;
pub mod shell;
pub mod task;
pub mod upload;

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

    /// Whether the log needs to be colored.
    #[clap(long, default_value = "auto", global = true)]
    color: ColorOutput,
}

/// Generates a completion script for a shell.
#[derive(Parser, Debug)]
pub struct CompletionCommand {
    /// The shell to generate a completion script for (defaults to 'bash').
    #[arg(short, long)]
    shell: Option<clap_complete::Shell>,
}

#[derive(Parser, Debug)]
pub enum Command {
    Completion(CompletionCommand),
    Init(init::Args),
    #[clap(alias = "a")]
    Add(add::Args),
    #[clap(alias = "r")]
    Run(run::Args),
    #[clap(alias = "s")]
    Shell(shell::Args),
    #[clap(alias = "g")]
    Global(global::Args),
    Auth(auth::Args),
    #[clap(alias = "i")]
    Install(install::Args),
    Task(task::Args),
    Info(info::Args),
    Upload(upload::Args),
    Search(search::Args),
}

fn completion(args: CompletionCommand) -> miette::Result<()> {
    let clap_shell = args
        .shell
        .or(clap_complete::Shell::from_env())
        .unwrap_or(clap_complete::Shell::Bash);
    clap_complete::generate(
        clap_shell,
        &mut Args::command(),
        "pixi",
        &mut std::io::stdout(),
    );

    // Create PS1 overwrite command
    // TODO: Also make this work for non bourne shells.
    let mut script = String::new();
    let shell = ShellEnum::from_str(clap_shell.to_string().as_str()).into_diagnostic()?;
    // Generate a shell agnostic command to add the PIXI_PROMPT to the PS1 variable.
    shell
        .set_env_var(
            &mut script,
            "PS1",
            format!(
                "{}{}",
                shell.format_env_var("PIXI_PROMPT"),
                shell.format_env_var("PS1")
            )
            .as_str(),
        )
        .unwrap();
    // Just like the clap autocompletion code write directly to the stdout
    std::io::stdout()
        .write_all(script.as_bytes())
        .into_diagnostic()?;

    Ok(())
}

pub async fn execute() -> miette::Result<()> {
    let args = Args::parse();
    let use_colors = use_color_output(&args);

    // Setup the default miette handler based on whether or not we want colors or not.
    miette::set_hook(Box::new(move |_| {
        Box::new(
            miette::MietteHandlerOpts::default()
                .color(use_colors)
                .build(),
        )
    }))?;

    // Enable disable colors for the colors crate
    console::set_colors_enabled(use_colors);
    console::set_colors_enabled_stderr(use_colors);

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
        .from_env()
        .into_diagnostic()?
        // filter logs from apple codesign because they are very noisy
        .add_directive("apple_codesign=off".parse().into_diagnostic()?)
        // set pixi's tracing level to warn
        .add_directive("pixi=warn".parse().into_diagnostic()?);

    // Setup the tracing subscriber
    tracing_subscriber::fmt()
        .with_ansi(use_colors)
        .with_env_filter(env_filter)
        .with_writer(IndicatifWriter::new(progress::global_multi_progress()))
        .without_time()
        .finish()
        .try_init()
        .into_diagnostic()?;

    // Execute the command
    execute_command(args.command).await
}

/// Execute the actual command
pub async fn execute_command(command: Command) -> miette::Result<()> {
    match command {
        Command::Completion(cmd) => completion(cmd),
        Command::Init(cmd) => init::execute(cmd).await,
        Command::Add(cmd) => add::execute(cmd).await,
        Command::Run(cmd) => run::execute(cmd).await,
        Command::Global(cmd) => global::execute(cmd).await,
        Command::Auth(cmd) => auth::execute(cmd).await,
        Command::Install(cmd) => install::execute(cmd).await,
        Command::Shell(cmd) => shell::execute(cmd).await,
        Command::Task(cmd) => task::execute(cmd),
        Command::Info(cmd) => info::execute(cmd).await,
        Command::Upload(cmd) => upload::execute(cmd).await,
        Command::Search(cmd) => search::execute(cmd).await,
    }
}

/// Whether to use colored log format.
/// Option `Auto` enables color output only if the logging is done to a terminal and  `NO_COLOR`
/// environment variable is not set.
#[derive(clap::ValueEnum, Debug, Clone, Default)]
pub enum ColorOutput {
    Always,
    Never,

    #[default]
    Auto,
}

/// Returns true if the output is considered to be a terminal.
fn is_terminal() -> bool {
    std::io::stderr().is_terminal()
}

/// Returns true if the log outputs should be colored or not.
fn use_color_output(args: &Args) -> bool {
    match args.color {
        ColorOutput::Always => true,
        ColorOutput::Never => false,
        ColorOutput::Auto => std::env::var_os("NO_COLOR").is_none() && is_terminal(),
    }
}
