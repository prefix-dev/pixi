use clap::Parser;
use clap::builder::styling::{AnsiColor, Color, Style};
use indicatif::ProgressDrawTarget;
use miette::IntoDiagnostic;
use pixi_consts::consts;
use pixi_progress::global_multi_progress;
use pixi_utils::indicatif::IndicatifWriter;
use std::{env, io::IsTerminal};
use tracing_subscriber::{
    EnvFilter, filter::LevelFilter, prelude::__tracing_subscriber_SubscriberExt,
    util::SubscriberInitExt,
};

pub mod add;
mod build;
pub mod clean;
pub mod cli_config;
pub mod command_info;
pub mod completion;
pub mod config;
pub mod exec;
pub mod global;
pub mod has_specs;
pub mod info;
pub mod init;
pub mod install;
pub mod list;
pub mod lock;
pub mod reinstall;
pub mod remove;
pub mod run;
pub mod search;
pub mod self_update;
pub mod shell;
pub mod shell_hook;
pub mod task;
pub mod tree;
pub mod update;
pub mod upgrade;
pub mod upload;
pub mod workspace;

#[derive(Parser, Debug)]
#[command(
    version(consts::PIXI_VERSION),
    about = format!("
Pixi [version {}] - Developer Workflow and Environment Management for Multi-Platform, Language-Agnostic Workspaces.

Pixi is a versatile developer workflow tool designed to streamline the management of your workspace's dependencies, tasks, and environments.
Built on top of the Conda ecosystem, Pixi offers seamless integration with the PyPI ecosystem.

Basic Usage:
    Initialize pixi for a workspace:
    $ pixi init
    $ pixi add python numpy pytest

    Run a task:
    $ pixi task add test 'pytest -s'
    $ pixi run test

Found a Bug or Have a Feature Request?
Open an issue at: https://github.com/prefix-dev/pixi/issues

Need Help?
Ask a question on the Prefix Discord server: https://discord.gg/kKV8ZxyzY4

For more information, see the documentation at: https://pixi.sh
", consts::PIXI_VERSION),
)]
#[clap(arg_required_else_help = true, styles=get_styles(), disable_help_flag = true, allow_external_subcommands = true)]
pub struct Args {
    #[command(subcommand)]
    command: Command,

    #[clap(flatten)]
    global_options: GlobalOptions,
}

#[derive(Debug, Parser)]
pub struct GlobalOptions {
    /// Display help information
    #[clap(long, short, global = true, action = clap::ArgAction::Help, help_heading = consts::CLAP_GLOBAL_OPTIONS)]
    help: Option<bool>,

    /// Increase logging verbosity (-v for warnings, -vv for info, -vvv for debug, -vvvv for trace)
    #[clap(short, long, action = clap::ArgAction::Count, global = true, help_heading = consts::CLAP_GLOBAL_OPTIONS)]
    verbose: u8,

    /// Decrease logging verbosity (quiet mode)
    #[clap(short, long, action = clap::ArgAction::Count, global = true, help_heading = consts::CLAP_GLOBAL_OPTIONS)]
    quiet: u8,

    /// Whether the log needs to be colored.
    #[clap(long, default_value = "auto", global = true, env = "PIXI_COLOR", help_heading = consts::CLAP_GLOBAL_OPTIONS)]
    color: ColorOutput,

    /// Hide all progress bars, always turned on if stderr is not a terminal.
    #[clap(long, default_value = "false", global = true, env = "PIXI_NO_PROGRESS", help_heading = consts::CLAP_GLOBAL_OPTIONS)]
    no_progress: bool,
}

impl Args {
    /// Whether to show progress bars or not, based on the terminal and the user's preference.
    fn no_progress(&self) -> bool {
        if !std::io::stderr().is_terminal() {
            true
        } else {
            self.global_options.no_progress
        }
    }

    /// Determine the log level filter based on verbose and quiet counts.
    fn log_level_filter(&self) -> LevelFilter {
        match (self.global_options.quiet, self.global_options.verbose) {
            // Quiet mode overrides verbose
            (q, _) if q > 0 => LevelFilter::OFF,
            // Custom verbosity levels
            (_, 0) => LevelFilter::ERROR, // Default
            (_, 1) => LevelFilter::WARN,  // -v
            (_, 2) => LevelFilter::INFO,  // -vv
            (_, 3) => LevelFilter::DEBUG, // -vvv
            (_, _) => LevelFilter::TRACE, // -vvvv+
        }
    }
}

#[derive(Parser, Debug)]
pub enum Command {
    // Commands in alphabetical order
    #[clap(visible_alias = "a")]
    Add(add::Args),
    Auth(rattler::cli::auth::Args),
    Build(build::Args),
    Clean(clean::Args),
    Completion(completion::Args),
    Config(config::Args),
    #[clap(visible_alias = "x")]
    Exec(exec::Args),
    #[clap(visible_alias = "g")]
    Global(global::Args),
    Info(info::Args),
    Init(init::Args),
    #[clap(visible_alias = "i")]
    Install(install::Args),
    #[clap(visible_alias = "ls")]
    List(list::Args),
    Lock(lock::Args),
    Reinstall(reinstall::Args),
    #[clap(visible_alias = "rm")]
    Remove(remove::Args),
    #[clap(visible_alias = "r")]
    Run(run::Args),
    Search(search::Args),
    #[cfg_attr(not(feature = "self_update"), clap(hide = true))]
    #[cfg_attr(feature = "self_update", clap(hide = false))]
    SelfUpdate(self_update::Args),
    #[clap(visible_alias = "s")]
    Shell(shell::Args),
    ShellHook(shell_hook::Args),
    Task(task::Args),
    #[clap(visible_alias = "t")]
    Tree(tree::Args),
    Update(update::Args),
    Upgrade(upgrade::Args),
    Upload(upload::Args),
    #[clap(alias = "project")]
    Workspace(workspace::Args),
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Parser, Debug, Default, Copy, Clone)]
#[group(multiple = false)]
/// Lock file usage from the CLI
pub struct LockFileUsageConfig {
    /// Install the environment as defined in the lockfile, doesn't update
    /// lockfile if it isn't up-to-date with the manifest file.
    #[clap(long, conflicts_with = "locked", env = "PIXI_FROZEN", help_heading = consts::CLAP_UPDATE_OPTIONS)]
    pub frozen: bool,
    /// Check if lockfile is up-to-date before installing the environment,
    /// aborts when lockfile isn't up-to-date with the manifest file.
    #[clap(long, conflicts_with = "frozen", env = "PIXI_LOCKED", help_heading = consts::CLAP_UPDATE_OPTIONS)]
    pub locked: bool,
}

impl From<LockFileUsageConfig> for crate::environment::LockFileUsage {
    fn from(value: LockFileUsageConfig) -> Self {
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
    set_console_colors(&args);
    let use_colors = console::colors_enabled_stderr();
    let in_ci = matches!(env::var("CI").as_deref(), Ok("1" | "true"));
    let no_wrap = matches!(env::var("PIXI_NO_WRAP").as_deref(), Ok("1" | "true"));
    // Set up the default miette handler based on whether we want colors or not.
    miette::set_hook(Box::new(move |_| {
        Box::new(
            miette::MietteHandlerOpts::default()
                .color(use_colors)
                // Don't wrap lines in CI environments or when explicitly specified to avoid
                // breaking logs and tests.
                .wrap_lines(!in_ci && !no_wrap)
                .build(),
        )
    }))?;

    // Hide all progress bars if the user requested it.
    if args.no_progress() {
        global_multi_progress().set_draw_target(ProgressDrawTarget::hidden());
    }

    let (low_level_filter, level_filter, pixi_level) = match args.log_level_filter() {
        LevelFilter::OFF => (LevelFilter::OFF, LevelFilter::OFF, LevelFilter::OFF),
        LevelFilter::ERROR => (LevelFilter::ERROR, LevelFilter::ERROR, LevelFilter::WARN),
        LevelFilter::WARN => (LevelFilter::WARN, LevelFilter::WARN, LevelFilter::INFO),
        LevelFilter::INFO => (LevelFilter::WARN, LevelFilter::INFO, LevelFilter::DEBUG),
        LevelFilter::DEBUG => (LevelFilter::INFO, LevelFilter::DEBUG, LevelFilter::TRACE),
        LevelFilter::TRACE => (LevelFilter::TRACE, LevelFilter::TRACE, LevelFilter::TRACE),
    };

    let env_filter = EnvFilter::builder()
        .with_default_directive(level_filter.into())
        .from_env()
        .into_diagnostic()?
        // filter logs from apple codesign because they are very noisy
        .add_directive("apple_codesign=off".parse().into_diagnostic()?)
        .add_directive(format!("pixi={}", pixi_level).parse().into_diagnostic()?)
        .add_directive(
            format!("pixi_command_dispatcher={}", pixi_level)
                .parse()
                .into_diagnostic()?,
        )
        .add_directive(
            format!("resolvo={}", low_level_filter)
                .parse()
                .into_diagnostic()?,
        );

    // Set up the tracing subscriber
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_ansi(use_colors)
        .with_target(pixi_level >= LevelFilter::INFO)
        .with_writer(IndicatifWriter::new(pixi_progress::global_multi_progress()))
        .without_time();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .init();

    // Execute the command
    execute_command(args.command, &args.global_options).await
}

/// Execute the actual command
pub async fn execute_command(
    command: Command,
    global_options: &GlobalOptions,
) -> miette::Result<()> {
    match command {
        Command::Completion(cmd) => completion::execute(cmd),
        Command::Config(cmd) => config::execute(cmd).await,
        Command::Init(cmd) => init::execute(cmd).await,
        Command::Add(cmd) => add::execute(cmd).await,
        Command::Clean(cmd) => clean::execute(cmd).await,
        Command::Run(cmd) => run::execute(cmd).await,
        Command::Global(cmd) => global::execute(cmd).await,
        Command::Auth(cmd) => rattler::cli::auth::execute(cmd).await.into_diagnostic(),
        Command::Install(cmd) => install::execute(cmd).await,
        Command::Reinstall(cmd) => reinstall::execute(cmd).await,
        Command::Shell(cmd) => shell::execute(cmd).await,
        Command::ShellHook(cmd) => shell_hook::execute(cmd).await,
        Command::Task(cmd) => task::execute(cmd).await,
        Command::Info(cmd) => info::execute(cmd).await,
        Command::Upload(cmd) => upload::execute(cmd).await,
        Command::Search(cmd) => search::execute(cmd).await,
        Command::Workspace(cmd) => workspace::execute(cmd).await,
        Command::Remove(cmd) => remove::execute(cmd).await,
        #[cfg(feature = "self_update")]
        Command::SelfUpdate(cmd) => self_update::execute(cmd, global_options).await,
        #[cfg(not(feature = "self_update"))]
        Command::SelfUpdate(cmd) => self_update::execute_stub(cmd, global_options).await,
        Command::List(cmd) => list::execute(cmd).await,
        Command::Tree(cmd) => tree::execute(cmd).await,
        Command::Update(cmd) => update::execute(cmd).await,
        Command::Upgrade(cmd) => upgrade::execute(cmd).await,
        Command::Lock(cmd) => lock::execute(cmd).await,
        Command::Exec(args) => exec::execute(args).await,
        Command::Build(args) => build::execute(args).await,
        Command::External(args) => command_info::execute_external_command(args),
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

fn set_console_colors(args: &Args) {
    // Honor FORCE_COLOR and NO_COLOR environment variables.
    // Those take precedence over the CLI flag and PIXI_COLOR
    let color = match env::var("FORCE_COLOR") {
        Ok(_) => &ColorOutput::Always,
        Err(_) => match env::var("NO_COLOR") {
            Ok(_) => &ColorOutput::Never,
            Err(_) => &args.global_options.color,
        },
    };

    match color {
        ColorOutput::Always => {
            console::set_colors_enabled(true);
            console::set_colors_enabled_stderr(true);
        }
        ColorOutput::Never => {
            console::set_colors_enabled(false);
            console::set_colors_enabled_stderr(false);
        }
        ColorOutput::Auto => {} // Let `console` detect if colors should be enabled
    };
}

pub fn get_styles() -> clap::builder::Styles {
    clap::builder::Styles::styled()
        .usage(
            Style::new()
                .bold()
                .underline()
                .fg_color(Some(Color::Ansi(AnsiColor::BrightGreen))),
        )
        .header(
            Style::new()
                .bold()
                .underline()
                .fg_color(Some(Color::Ansi(AnsiColor::BrightGreen))),
        )
        .literal(Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightCyan))))
        .invalid(
            Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Red))),
        )
        .error(
            Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Red))),
        )
        .valid(
            Style::new()
                .bold()
                .underline()
                .fg_color(Some(Color::Ansi(AnsiColor::Green))),
        )
        .placeholder(Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightCyan))))
}
