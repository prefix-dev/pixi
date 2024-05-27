use std::{env, io::IsTerminal};

use clap::Parser;
use clap_verbosity_flag::Verbosity;
use indicatif::ProgressDrawTarget;
use miette::IntoDiagnostic;
use tracing_subscriber::{
    filter::LevelFilter, prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt,
    EnvFilter,
};

use super::util::IndicatifWriter;
use crate::{progress, progress::global_multi_progress};

pub mod add;
pub mod completion;
pub mod config;
pub mod global;
pub mod has_specs;
pub mod info;
pub mod init;
pub mod install;
pub mod list;
pub mod project;
pub mod remove;
pub mod run;
pub mod search;
pub mod self_update;
pub mod shell;
pub mod shell_hook;
pub mod task;
pub mod tree;
pub mod update;
pub mod upload;

#[derive(Parser, Debug)]
#[command(
    version,
    about = "
Pixi [version 0.23.0] - Developer Workflow and Environment Management for Multi-Platform, Language-Agnostic Projects.

Pixi is a versatile developer workflow tool designed to streamline the management of your project's dependencies, tasks, and environments.
Built on top of the Conda ecosystem, Pixi offers seamless integration with the PyPI ecosystem.

Basic Usage:
    Initialize pixi for a project:
    $ pixi init
    $ pixi add python numpy pytest

    Run a task:
    $ pixi add task test 'pytest -s'
    $ pixi run test

Found a Bug or Have a Feature Request?
Open an issue at: https://github.com/prefix-dev/pixi/issues

Need Help?
Ask a question on the Prefix Discord server: https://discord.gg/kKV8ZxyzY4

For more information, see the documentation at: https://pixi.sh
"
)]
#[clap(arg_required_else_help = true)]
struct Args {
    #[command(subcommand)]
    command: Command,

    /// The verbosity level
    /// (-v for warning, -vv for info, -vvv for debug, -vvvv for trace, -q for
    /// quiet)
    #[command(flatten)]
    verbose: Verbosity,

    /// Whether the log needs to be colored.
    #[clap(long, default_value = "auto", global = true, env = "PIXI_COLOR")]
    color: ColorOutput,

    /// Hide all progress bars
    #[clap(long, default_value = "false", global = true, env = "PIXI_NO_PROGRESS")]
    no_progress: bool,
}

#[derive(Parser, Debug)]
pub enum Command {
    Init(init::Args),

    // Installation commands
    #[clap(visible_alias = "a")]
    Add(add::Args),
    #[clap(visible_alias = "rm")]
    Remove(remove::Args),
    #[clap(visible_alias = "i")]
    Install(install::Args),
    Update(update::Args),

    // Execution commands
    #[clap(visible_alias = "r")]
    Run(run::Args),
    #[clap(visible_alias = "s")]
    Shell(shell::Args),
    ShellHook(shell_hook::Args),

    // Project modification commands
    Project(project::Args),
    Task(task::Args),

    // Environment inspection
    #[clap(visible_alias = "ls")]
    List(list::Args),
    #[clap(visible_alias = "t")]
    Tree(tree::Args),

    // Global level commands
    #[clap(visible_alias = "g")]
    Global(global::Args),
    Auth(rattler::cli::auth::Args),
    Config(config::Args),
    Info(info::Args),
    Upload(upload::Args),
    Search(search::Args),
    SelfUpdate(self_update::Args),
    Completion(completion::Args),
}

#[derive(Parser, Debug, Default, Copy, Clone)]
#[group(multiple = false)]
/// Lock file usage from the CLI
pub struct LockFileUsageArgs {
    // Install the environment as defined in the lockfile, doesn't update lockfile if it isn't
    // up-to-date with the manifest file.
    #[clap(long, conflicts_with = "locked", env = "PIXI_FROZEN")]
    pub frozen: bool,
    /// Check if lockfile is up-to-date before installing the environment,
    /// aborts when lockfile isn't up-to-date with the manifest file.
    #[clap(long, conflicts_with = "frozen", env = "PIXI_LOCKED")]
    pub locked: bool,
}

impl From<LockFileUsageArgs> for crate::environment::LockFileUsage {
    fn from(value: LockFileUsageArgs) -> Self {
        if value.frozen {
            Self::Frozen
        } else if value.locked {
            Self::Locked
        } else {
            Self::Update
        }
    }
}

pub async fn execute() -> miette::Result<()> {
    let args = Args::parse();
    let use_colors = use_color_output(&args);

    // Set up the default miette handler based on whether we want colors or not.
    miette::set_hook(Box::new(move |_| {
        Box::new(
            miette::MietteHandlerOpts::default()
                .color(use_colors)
                .build(),
        )
    }))?;

    // Honor FORCE_COLOR and NO_COLOR environment variables.
    // Those take precedence over the CLI flag and PIXI_COLOR
    let use_colors = match env::var("FORCE_COLOR") {
        Ok(_) => true,
        Err(_) => match env::var("NO_COLOR") {
            Ok(_) => false,
            Err(_) => use_colors,
        },
    };

    // Enable disable colors for the colors crate
    console::set_colors_enabled(use_colors);
    console::set_colors_enabled_stderr(use_colors);

    // Hide all progress bars if the user requested it.
    if args.no_progress {
        global_multi_progress().set_draw_target(ProgressDrawTarget::hidden());
    }

    let (low_level_filter, level_filter, pixi_level) = match args.verbose.log_level_filter() {
        clap_verbosity_flag::LevelFilter::Off => {
            (LevelFilter::OFF, LevelFilter::OFF, LevelFilter::OFF)
        }
        clap_verbosity_flag::LevelFilter::Error => {
            (LevelFilter::ERROR, LevelFilter::ERROR, LevelFilter::WARN)
        }
        clap_verbosity_flag::LevelFilter::Warn => {
            (LevelFilter::WARN, LevelFilter::WARN, LevelFilter::INFO)
        }
        clap_verbosity_flag::LevelFilter::Info => {
            (LevelFilter::WARN, LevelFilter::INFO, LevelFilter::INFO)
        }
        clap_verbosity_flag::LevelFilter::Debug => {
            (LevelFilter::INFO, LevelFilter::DEBUG, LevelFilter::DEBUG)
        }
        clap_verbosity_flag::LevelFilter::Trace => {
            (LevelFilter::TRACE, LevelFilter::TRACE, LevelFilter::TRACE)
        }
    };

    let env_filter = EnvFilter::builder()
        .with_default_directive(level_filter.into())
        .from_env()
        .into_diagnostic()?
        // filter logs from apple codesign because they are very noisy
        .add_directive("apple_codesign=off".parse().into_diagnostic()?)
        .add_directive(format!("pixi={}", pixi_level).parse().into_diagnostic()?)
        .add_directive(
            format!("resolvo={}", low_level_filter)
                .parse()
                .into_diagnostic()?,
        )
        .add_directive(
            format!("rattler_installs_packages={}", pixi_level)
                .parse()
                .into_diagnostic()?,
        )
        .add_directive(
            format!(
                "rattler_networking::authentication_storage::backends::file={}",
                LevelFilter::OFF
            )
            .parse()
            .into_diagnostic()?,
        )
        .add_directive(
            format!(
                "rattler_networking::authentication_storage::storage={}",
                LevelFilter::OFF
            )
            .parse()
            .into_diagnostic()?,
        );

    // Set up the tracing subscriber
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_ansi(use_colors)
        .with_writer(IndicatifWriter::new(progress::global_multi_progress()))
        .without_time();

    cfg_if::cfg_if! {
        if #[cfg(feature = "console-subscriber")]
        {
            let console_layer = console_subscriber::spawn();
            tracing_subscriber::registry()
                .with(console_layer)
                .with(env_filter)
                .with(fmt_layer)
                .init();
        } else {
            tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();
        }
    }

    // Execute the command
    execute_command(args.command).await
}

/// Execute the actual command
pub async fn execute_command(command: Command) -> miette::Result<()> {
    match command {
        Command::Completion(cmd) => completion::execute(cmd),
        Command::Config(cmd) => config::execute(cmd).await,
        Command::Init(cmd) => init::execute(cmd).await,
        Command::Add(cmd) => add::execute(cmd).await,
        Command::Run(cmd) => run::execute(cmd).await,
        Command::Global(cmd) => global::execute(cmd).await,
        Command::Auth(cmd) => rattler::cli::auth::execute(cmd).await.into_diagnostic(),
        Command::Install(cmd) => install::execute(cmd).await,
        Command::Shell(cmd) => shell::execute(cmd).await,
        Command::ShellHook(cmd) => shell_hook::execute(cmd).await,
        Command::Task(cmd) => task::execute(cmd),
        Command::Info(cmd) => info::execute(cmd).await,
        Command::Upload(cmd) => upload::execute(cmd).await,
        Command::Search(cmd) => search::execute(cmd).await,
        Command::Project(cmd) => project::execute(cmd).await,
        Command::Remove(cmd) => remove::execute(cmd).await,
        Command::SelfUpdate(cmd) => self_update::execute(cmd).await,
        Command::List(cmd) => list::execute(cmd).await,
        Command::Tree(cmd) => tree::execute(cmd).await,
        Command::Update(cmd) => update::execute(cmd).await,
    }
}

/// Whether to use colored log format.
/// Option `Auto` enables color output only if the logging is done to a terminal
/// and  `NO_COLOR` environment variable is not set.
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
