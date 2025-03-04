use clap::{Command, CommandFactory};
use itertools::Itertools;
use std::fmt::Write;
use std::fs;
use std::path::Path;
use clap::builder::Str;

/// This tool generates the documentation for the pixi cli.
/// The implementation works as follows:
/// - The clap command is loaded from the pixi crate.
/// - We create a table of contents from the command.
/// - We generate a markdown file per command.
/// - The commands split into modules are split into directories in the markdown output directory
///

fn main() {
    let command = get_command();
    // Get version
    println!("Version: {}\n", command.get_version().unwrap());

    // Process subcommands
    process_subcommands(&command, vec![]);
}

fn process_subcommands(command: &Command, parent_path: Vec<String>) {
    // Create path for current command
    let mut current_path = parent_path.clone();
    current_path.push(command.get_name().to_string());

    // Generate file name for current command
    let command_file_name = format!("{}.md", current_path.join("/"));
    let command_file_path = Path::new(&command_file_name);

    println!("Processing command: {}", command_file_path.display());

    // Create directories and write file for current command
    fs::create_dir_all(command_file_path.parent().unwrap())
        .expect("Failed to create directories");
    fs::write(
        command_file_path,
        subcommand_to_md(&parent_path, command)
    )
        .expect("Failed to write command file");

    // Process all subcommands
    for subcommand in command.get_subcommands() {
        process_subcommands(subcommand, current_path.clone());
    }
}

fn subcommand_to_md(parents: &[String], command: &Command) -> String {
    let mut buffer = String::new();
    let full_name = format!("{} {}", parents.join(" "), command.get_name());
    // ---------- Name ----------
    write!(
        buffer,
        "# `{}`\n",
        full_name
    )
    .unwrap();

    // ---------- Short about ----------
    write!(buffer, "## About\n").unwrap();
    write!(buffer, "{}\n", command.get_about().unwrap()).unwrap();

    // ---------- Synopsis ----------
    write!(buffer, "## Synopsis\n").unwrap();
    write!(buffer, "```shell\n").unwrap();
    write!(buffer, "{}", full_name).unwrap();
    // Add positionals
    let positionals = command
        .get_positionals()
        .flat_map(|positional| {
            positional
                .get_value_names()
                .map(|value_names| format!("[{}]", value_names.join(" ")))
        })
        .collect::<Vec<_>>();
    if positionals.len() > 0 {
        write!(buffer, " {}", positionals.into_iter().join(" ")).unwrap();
    }
    write!(buffer, "\n```\n").unwrap();

    // ---------- Subcommands ----------
    if command.get_subcommands().next().is_some() {
        write!(buffer, "## Subcommands\n").unwrap();
        command.get_subcommands().for_each(|subcommand| {
            write!(
                buffer,
                "- [{}](#{})\n",
                subcommand.get_name(),
                subcommand.get_name()
            )
            .unwrap();
        });
    }

    // ---------- Positionals ----------
    if command.get_positionals().next().is_some() {
        write!(buffer, "## Positionals\n").unwrap();
        command.get_positionals().for_each(|positional| {
            write!(
                buffer,
                "- **{}**",
                positional.get_value_names().unwrap_or(&[Str::from("")]).join(" ")
            )
            .unwrap();

            if let Some(help) = positional.get_long_help() {
                write!(buffer, ": {}\n", help).unwrap();
            } else if let Some(help) = positional.get_help() {
                write!(buffer, ": {}\n", help).unwrap();
            }
        });
    }

    // ---------- Options ----------
    if command.get_opts().next().is_some() {
        write!(buffer, "## Options\n").unwrap();
        // First list all non global options
        let opts = command
            .get_opts()
            .sorted_by(|a, b| {
                a.get_long()
                    .unwrap_or_default()
                    .cmp(&b.get_long().unwrap_or_default())
            })
            .sorted_by(|a, b| a.is_global_set().cmp(&b.is_global_set()))
            .collect::<Vec<_>>();

        opts.iter().for_each(|argument| {
            if argument.is_hide_set() {
                return;
            }
            let global = if argument.is_global_set() {
                "global: "
            } else {
                ""
            };

            write!(
                buffer,
                "- {}**`--{}`**",
                global,
                argument.get_long().unwrap_or_default()
            )
            .unwrap();
            if let Some(aliases) = argument.get_all_aliases() {
                write!(buffer, " (aliases: {})", aliases.join(", ")).unwrap();
            }
            write!(buffer, ": {}\n", argument.get_help().unwrap_or_default()).unwrap();
        });
    }
    // ---------- Long about ----------
    if let Some(help) = command.get_after_help() {
        write!(buffer, "## Help\n").unwrap();
        write!(buffer, "{}\n", help).unwrap();
    }

    buffer
}

fn get_command() -> Command {
    // Load the command from the pixi crate
    pixi::cli::Args::command()
}
