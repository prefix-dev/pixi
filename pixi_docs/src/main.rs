use clap::{Command, CommandFactory};

/// This tool generates the documentation for the pixi cli.
/// The implementation works as follows:
/// - The clap command is loaded from the pixi crate.
/// - We create a table of contents from the command.
/// - We generate a markdown file per command.
/// - The commands split into modules are split into directories in the markdown output directory
///



fn main() {
    let command = get_command();
    println!("{}", command.get_version().unwrap());
}

fn get_command() -> Command {
    // Load the command from the pixi crate
    pixi::cli::Args::command()

}
