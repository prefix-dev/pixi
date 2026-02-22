//! # Pixi CLI
//!
//! This module implements the CLI interface of Pixi.
//!
//! ## Structure
//!
//! - The [`Command`] enum defines the top-level commands available.
//! - The [`execute_command`] function matches on [`Command`] and calls the corresponding logic.
#![deny(clippy::dbg_macro, clippy::unwrap_used)]

use clap::builder::styling::{AnsiColor, Color, Style};
use clap::{CommandFactory, Parser};
use indicatif::ProgressDrawTarget;
use miette::IntoDiagnostic;
use pixi_consts::consts;
use pixi_core::environment::LockFileUsage;
use pixi_progress::global_multi_progress;

use std::{env, io::IsTerminal};
use tracing::level_filters::LevelFilter;

pub mod add;
pub mod build;
pub mod clean;
pub mod cli_config;
pub mod cli_interface;
pub mod command_info;
pub mod completion;
pub mod config;
pub mod exec;
pub mod global;
pub mod has_specs;
pub mod import;
pub mod info;
pub mod init;
pub mod install;
pub mod list;
pub mod lock;
pub(crate) mod match_spec_or_path;
pub mod reinstall;
pub mod remove;
pub mod run;
pub mod search;
pub mod self_update;
mod shared;
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
    name = "pixi",
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
    command: Option<Command>,

    #[clap(flatten)]
    global_options: GlobalOptions,

    /// List all installed commands (built-in and extensions)
    #[clap(long = "list", help_heading = consts::CLAP_GLOBAL_OPTIONS)]
    list: bool,
}

#[derive(Debug, Parser)]
pub struct GlobalOptions {
    /// Display help information
    #[clap(
        long,
        short,
        global = true,
        action = clap::ArgAction::Help,
        help_heading = consts::CLAP_GLOBAL_OPTIONS
    )]
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
    #[allow(unused)]
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
#[allow(clippy::large_enum_variant)]
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
    Import(import::Args),
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

/// Configuration for lock file usage, used by LockFileUpdateConfig
#[derive(Parser, Debug, Default, Clone)]
pub struct LockFileUsageConfig {
    /// Install the environment as defined in the lockfile, doesn't update
    /// lockfile if it isn't up-to-date with the manifest file.
    #[clap(
        long,
        env = "PIXI_FROZEN",
        help_heading = consts::CLAP_UPDATE_OPTIONS
    )]
    pub frozen: bool,
    /// Check if lockfile is up-to-date before installing the environment,
    /// aborts when lockfile isn't up-to-date with the manifest file.
    #[clap(
        long,
        env = "PIXI_LOCKED",
        help_heading = consts::CLAP_UPDATE_OPTIONS
    )]
    pub locked: bool,
}

impl LockFileUsageConfig {
    pub fn to_usage(&self) -> LockFileUsage {
        if self.locked {
            LockFileUsage::Locked
        } else if self.frozen {
            LockFileUsage::Frozen
        } else {
            LockFileUsage::Update
        }
    }
}

pub async fn execute() -> miette::Result<()> {
    let args = Args::parse();

    // Extract values we need before moving args
    let no_progress = args.no_progress();

    set_console_colors(&args);

    let use_colors = console::colors_enabled_stderr();
    let in_ci = matches!(env::var("CI").as_deref(), Ok("1" | "true"));
    let no_wrap = matches!(env::var("PIXI_NO_WRAP").as_deref(), Ok("1" | "true"));
    // Set up the default miette handler based on whether we want colors or not.
    miette::set_hook(Box::new(move |_| {
        Box::new(
            miette::MietteHandlerOpts::default()
                .color(use_colors)
                .with_syntax_highlighting(miette_arborium::MietteHighlighter::new())
                // Don't wrap lines in CI environments or when explicitly specified to avoid
                // breaking logs and tests.
                .wrap_lines(!in_ci && !no_wrap)
                .build(),
        )
    }))?;

    // Hide all progress bars if the user requested it.
    if no_progress {
        global_multi_progress().set_draw_target(ProgressDrawTarget::hidden());
    }

    // Handle `--list`: print installed commands and exit 0
    if args.list {
        print_installed_commands();
        return Ok(());
    }

    // Setup logging for the application.
    setup_logging(&args, use_colors)?;

    let (Some(command), global_options) = (args.command, args.global_options) else {
        // match CI expectations
        std::process::exit(2);
    };

    // Execute the command
    execute_command(command, &global_options).await
}

#[cfg(feature = "console-subscriber")]
fn setup_logging(_args: &Args, _use_colors: bool) -> miette::Result<()> {
    console_subscriber::init();
    Ok(())
}

#[cfg(not(feature = "console-subscriber"))]
fn setup_logging(args: &Args, use_colors: bool) -> miette::Result<()> {
    use pixi_utils::indicatif::IndicatifWriter;
    use tracing_subscriber::{
        EnvFilter, filter::LevelFilter, prelude::__tracing_subscriber_SubscriberExt,
        util::SubscriberInitExt,
    };

    let (low_level_filter, level_filter, pixi_level) = match args.log_level_filter() {
        LevelFilter::OFF => (LevelFilter::OFF, LevelFilter::OFF, LevelFilter::OFF),
        LevelFilter::ERROR => (LevelFilter::ERROR, LevelFilter::ERROR, LevelFilter::WARN),
        LevelFilter::WARN => (LevelFilter::WARN, LevelFilter::WARN, LevelFilter::INFO),
        LevelFilter::INFO => (LevelFilter::WARN, LevelFilter::INFO, LevelFilter::DEBUG),
        LevelFilter::DEBUG => (LevelFilter::INFO, LevelFilter::DEBUG, LevelFilter::TRACE),
        LevelFilter::TRACE => (LevelFilter::TRACE, LevelFilter::TRACE, LevelFilter::TRACE),
    };

    // Check if CLI verbosity flags were explicitly set
    let cli_verbosity_set = args.global_options.verbose > 0 || args.global_options.quiet > 0;

    // Build the filter with appropriate precedence:
    // - If CLI flags are set (--quiet/-v), use them and ignore RUST_LOG
    // - If no CLI flags, try and RUST_LOG if available
    let env_filter = if cli_verbosity_set {
        // CLI flags take precedence i.e. ignore RUST_LOG
        EnvFilter::builder()
            .with_default_directive(level_filter.into())
            .parse(format!(
                "apple_codesign=off,pixi={pixi_level},pixi_command_dispatcher={pixi_level},pixi_core={pixi_level},uv_resolver={pixi_level},resolvo={low_level_filter}"
            ))
            .into_diagnostic()?
    } else {
        // No CLI flags - use RUST_LOG if set
        // Parse RUST_LOG because we need to set it other our other directives
        let env_directives = env::var("RUST_LOG").unwrap_or_default();
        let original_directives = format!(
            "apple_codesign=off,pixi={pixi_level},pixi_command_dispatcher={pixi_level},pixi_core={pixi_level},uv_resolver={pixi_level},resolvo={low_level_filter}",
        );
        // Concatenate both directives where the LOG overrides the potential original directives
        let final_directives = if env_directives.is_empty() {
            original_directives
        } else {
            format!("{original_directives},{env_directives}")
        };

        EnvFilter::builder()
            .with_default_directive(level_filter.into())
            .parse(&final_directives)
            .into_diagnostic()?
    };

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
    Ok(())
}

/// Maps command enum variants to their actual function handlers.
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
        Command::Import(cmd) => import::execute(cmd).await,
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

/// Print all installed commands (built-in and external extensions)
fn print_installed_commands() {
    use std::collections::BTreeMap;

    let use_colors = console::colors_enabled_stderr();

    // Header
    if use_colors {
        println!(
            "{}",
            console::style("Installed Commands:")
                .bold()
                .underlined()
                .bright()
                .green()
        );
    } else {
        println!("Installed Commands:");
    }

    // Built-in commands
    let mut builtins: BTreeMap<String, Option<String>> = BTreeMap::new();
    for sc in Args::command().get_subcommands() {
        builtins.insert(
            sc.get_name().to_string(),
            sc.get_about().map(|s| s.to_string()),
        );
    }

    for (name, about) in builtins {
        let summary = about
            .as_deref()
            .and_then(|s| s.lines().next())
            .unwrap_or("");
        if use_colors {
            println!(
                "    {} {}",
                console::style(format!("{name:<20}")).bright().cyan(),
                summary
            );
        } else {
            println!("    {name:<20} {summary}");
        }
    }

    // External commands (pixi-*)
    let mut external_names: Vec<String> =
        command_info::find_external_commands().into_keys().collect();
    external_names.sort();

    for name in external_names {
        let via = format!("(via pixi-{name})");
        if use_colors {
            println!(
                "    {} {}",
                console::style(format!("{name:<20}")).bright().cyan(),
                via
            );
        } else {
            println!("    {name:<20} {via}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clap_boolean_env_var_behavior() {
        // Test PIXI_FROZEN=true
        temp_env::with_var("PIXI_FROZEN", Some("true"), || {
            let result = LockFileUsageConfig::try_parse_from(["test"]);
            assert!(result.is_ok());
            let parsed = result.unwrap();
            assert!(
                parsed.frozen,
                "Expected PIXI_FROZEN=true to set frozen=true"
            );
            let usage = parsed.to_usage();
            assert!(matches!(
                usage,
                pixi_core::environment::LockFileUsage::Frozen
            ));
        });

        // Test PIXI_FROZEN=false
        temp_env::with_var("PIXI_FROZEN", Some("false"), || {
            let result = LockFileUsageConfig::try_parse_from(["test"]);
            assert!(result.is_ok());
            let parsed = result.unwrap();
            assert!(
                !parsed.frozen,
                "Expected PIXI_FROZEN=false to set frozen=false"
            );
            let usage = parsed.to_usage();
            assert!(matches!(
                usage,
                pixi_core::environment::LockFileUsage::Update
            ));
        });

        // Test unset
        temp_env::with_var_unset("PIXI_FROZEN", || {
            let result = LockFileUsageConfig::try_parse_from(["test"]);
            assert!(result.is_ok());
            let parsed = result.unwrap();
            assert!(!parsed.frozen, "Expected unset PIXI_FROZEN to be false");
        });
    }

    #[test]
    fn test_cli_args_override_env_vars() {
        // Test that CLI arguments take precedence over environment variables
        temp_env::with_var("PIXI_FROZEN", Some("false"), || {
            let result = LockFileUsageConfig::try_parse_from(["test", "--frozen"]);
            assert!(result.is_ok());
            let parsed = result.unwrap();
            let usage = parsed.to_usage();
            assert!(matches!(
                usage,
                pixi_core::environment::LockFileUsage::Frozen
            ));
        });
    }

    #[test]
    fn test_locked_env_and_precedence() {
        // PIXI_FROZEN=true and PIXI_LOCKED=false should work
        temp_env::with_vars(
            vec![
                ("PIXI_FROZEN", Some("true")),
                ("PIXI_LOCKED", Some("false")),
            ],
            || {
                let parsed = LockFileUsageConfig::try_parse_from(["test"]).unwrap();
                let usage = parsed.to_usage();
                assert!(matches!(
                    usage,
                    pixi_core::environment::LockFileUsage::Frozen
                ));
            },
        );

        // PIXI_FROZEN=true and PIXI_LOCKED=true should select Locked (locked wins)
        temp_env::with_vars(
            vec![("PIXI_FROZEN", Some("true")), ("PIXI_LOCKED", Some("true"))],
            || {
                let parsed = LockFileUsageConfig::try_parse_from(["test"]).unwrap();
                let usage = parsed.to_usage();
                assert!(matches!(
                    usage,
                    pixi_core::environment::LockFileUsage::Locked
                ));
            },
        );
    }
}
